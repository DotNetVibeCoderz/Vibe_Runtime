//! Tiering: interpret first, compile what turns out to be hot.
//!
//! Compiling costs time and memory, and most methods in a program run once.
//! So nothing is compiled up front. Each call to a compilable method increments
//! a counter, and the [`JitTier::threshold`]th call triggers compilation; from
//! then on the method runs as machine code.
//!
//! A method the backend declines is recorded once and never reconsidered, so
//! the cost of declining is a hash lookup rather than a repeated analysis.

use crate::x64::{NativeMethod, X64Backend};
use crate::Compiler;
use rustclr_core::{
    ArrayStorage, ClrArray, ClrExceptionKind, ExecResult, ExecutionError, Loader, MethodId,
    NativeTier, Value,
};
use rustclr_core::gc::{Handle, SharedHeap};
use rustclr_metadata::TypeSig;
use std::collections::{HashMap, HashSet};

/// The interpret-then-compile policy.
pub struct JitTier {
    backend: X64Backend,
    /// Calls before a method is compiled.
    pub threshold: u32,
    counts: HashMap<MethodId, u32>,
    compiled: HashMap<MethodId, NativeMethod>,
    /// Methods the backend cannot take, so they are not analysed again.
    declined: HashSet<MethodId>,
    /// Whether a method's body loops, decoded once and remembered.
    loops: HashMap<MethodId, bool>,
    /// Methods whose call tables are being resolved, to break a cycle.
    resolving: HashSet<MethodId>,
    /// Methods whose call tables are filled in and ready to be called through.
    resolved: HashSet<MethodId>,
    /// Scratch buffers for marshalling one call's arguments.
    ///
    /// Two heap allocations per call is not much until a benchmark makes a
    /// quarter of a million of them. They are *moved out* of the tier for the
    /// duration of a call rather than borrowed, so a nested `try_execute` —
    /// which nothing does today — would allocate its own rather than write
    /// into a buffer whose addresses compiled code is still holding.
    scratch_marshalled: Vec<i64>,
    scratch_descriptors: Vec<[i64; 2]>,
}

impl Default for JitTier {
    fn default() -> Self {
        Self::new()
    }
}

impl JitTier {
    pub fn new() -> Self {
        Self::with_threshold(32)
    }

    /// A threshold of zero compiles on the first call, which is what the
    /// tests and `rustnet jit` want.
    pub fn with_threshold(threshold: u32) -> Self {
        Self {
            backend: X64Backend::new(),
            threshold,
            counts: HashMap::new(),
            compiled: HashMap::new(),
            declined: HashSet::new(),
            loops: HashMap::new(),
            resolving: HashSet::new(),
            resolved: HashSet::new(),
            scratch_marshalled: Vec::new(),
            scratch_descriptors: Vec::new(),
        }
    }

    /// Whether the backend inlines small callees. See [`X64Backend::inline`].
    pub fn set_inline(&mut self, inline: bool) {
        self.backend.inline = inline;
    }

    pub fn compiled_count(&self) -> usize {
        self.compiled.len()
    }

    pub fn declined_count(&self) -> usize {
        self.declined.len()
    }

    /// Compiles `method` now, whatever its call count.
    ///
    /// Returns `false` when the backend declines it — which is not an error.
    pub fn compile_now(&mut self, loader: &Loader, method: MethodId) -> bool {
        if self.compiled.contains_key(&method) {
            return true;
        }
        if self.declined.contains(&method) {
            return false;
        }
        if !self.backend.can_compile(&loader.registry, method) {
            self.declined.insert(method);
            return false;
        }
        match self.backend.compile_native(loader, method) {
            Ok(native) => {
                self.compiled.insert(method, native);
                true
            }
            Err(_) => {
                // A method that passes `can_compile` but fails to emit is a
                // backend gap, not a program error: record it and interpret.
                self.declined.insert(method);
                false
            }
        }
    }
}

impl NativeTier for JitTier {
    fn name(&self) -> &'static str {
        "x86-64 baseline (tiered)"
    }

    fn try_execute(
        &mut self,
        loader: &Loader,
        heap: &SharedHeap,
        method: MethodId,
        args: &[Value],
        mutators: &rustclr_core::gc::safepoint::Mutators,
        interpreter_roots: &mut dyn FnMut() -> Vec<rustclr_core::gc::Handle>,
    ) -> Option<ExecResult<Option<Value>>> {
        if self.declined.contains(&method) {
            return None;
        }

        if !self.compiled.contains_key(&method) {
            let count = self.counts.entry(method).or_insert(0);
            *count += 1;
            // A call count is a proxy for "this method is worth compiling", and
            // it is the wrong proxy for a method that loops: `Sieve` and
            // `Matrix` are each called *once* and run for seconds, so they
            // never crossed the threshold however long they ran. Compiling on
            // the first call when the body contains a backward branch fixes
            // that, and costs a few microseconds when the loop turns out to be
            // short — against seconds when it is not.
            //
            // This is not on-stack replacement: the decision is made before the
            // call, so a method already running cannot be swapped. A loop-free
            // method still waits for the count, because compiling one that is
            // called once is pure loss.
            if *count < self.threshold.max(1) && !self.loops(loader, method) {
                return None;
            }
            if !self.compile_now(loader, method) {
                return None;
            }
        }

        // Every callee this body reaches has to be compiled before it runs,
        // because its calls are indirect through a table of their entries.
        // Done here rather than during compilation: two methods may call each
        // other, and compiling one to finish the other would not terminate.
        if !self.resolve_call_table(loader, method) {
            // A callee turned out not to compile after all. The caller was
            // emitted around a call that can never be filled in, so it is
            // discarded and the method is interpreted — which is a decision to
            // make once, not on every call.
            self.compiled.remove(&method);
            self.declined.insert(method);
            return None;
        }

        let native = self.compiled.get(&method)?;

        // An `int[]` argument becomes a two-word descriptor — data pointer and
        // length — and its argument slot carries the descriptor's address.
        // The descriptors live in this vector for exactly the duration of the
        // call, and it is sized up front so no reallocation can move one while
        // the compiled code holds its address.
        //
        // Taking a raw pointer into the managed heap is sound here for a reason
        // worth stating rather than assuming: nothing collects while compiled
        // code runs, and this collector never moves an object in any case.
        //
        // Compiled code can allocate now, which sounds like it breaks the
        // first half and does not: `Heap::alloc` records pressure and returns,
        // and it is the *interpreter* that collects, between instructions.
        // Compiled code has no between-instructions. See `crate::alloc`.
        let mut descriptors: Vec<[i64; 2]> = core::mem::take(&mut self.scratch_descriptors);
        descriptors.clear();
        descriptors.reserve(native.arg_count);
        // Array arguments reach compiled code as a raw pointer into the
        // array's storage. Nothing else roots them while the call runs — the
        // interpreter pushes no frame for a compiled method — so a collection
        // reached from inside the method would sweep the storage the pointer
        // names. They are pinned for the duration of the call instead.
        let mut argument_handles: Vec<rustclr_core::gc::Handle> = Vec::new();
        for i in 0..native.arg_count {
            if let Value::Obj(handle) = args.get(i)? {
                descriptors.push(int_array_descriptor(heap, *handle)?);
                argument_handles.push(*handle);
            }
        }

        let mut marshalled: Vec<i64> = core::mem::take(&mut self.scratch_marshalled);
        marshalled.clear();
        marshalled.reserve(native.arg_count + 3);
        let mut next = 0usize;
        for i in 0..native.arg_count {
            match args.get(i)? {
                Value::Obj(_) => {
                    let address = descriptors[next].as_ptr() as i64;
                    next += 1;
                    marshalled.push(address);
                }
                // Anything that is neither an integer nor an array means the
                // caller and the compiled signature disagree, which
                // `can_compile` should have prevented.
                other => marshalled.push(as_i64(other)?),
            }
        }
        // One slot past the arguments is the trap flag the emitted bounds
        // checks write to. Pushing it unconditionally also keeps the pointer
        // valid for a method with no arguments at all, which the emitted
        // prologue still dereferences.
        marshalled.push(0);

        // And one past *that* is the allocation context. The sink owns every
        // array the call allocates and drops them when it returns, which is
        // sound because a compiled method cannot return a reference — so
        // nothing it allocated can outlive the call.
        // Only a body that can allocate gets an allocation context. Building
        // one resolves three types by name and searches for three array
        // constructions, and a method that never allocates has no use for the
        // answer — which described almost every compiled method in the
        // benchmark suite, called a quarter of a million times each.
        let needs_context = native.allocates || !native.reference_slots.is_empty()
            || !argument_handles.is_empty();
        let mut sink = needs_context.then(|| self.array_types(loader)).flatten().map(
            |mut types| {
                // The classes this body names, in its own numbering. Copied per
                // call rather than shared, because the indices the emitted code
                // uses belong to this method and no other.
                types.objects = native.object_shapes.clone();
                // The starting field values for each of those classes,
                // resolved through the loader because that is where a
                // signature becomes a value.
                types.object_fields = native
                    .object_shapes
                    .iter()
                    .map(|(ty, _, _)| loader.default_instance_fields(*ty))
                    .collect();
                let mut sink = crate::alloc::AllocSink::new(heap.clone(), types);
                for handle in &argument_handles {
                    sink.borrow(*handle);
                }
                sink
            },
        );
        marshalled.push(
            sink.as_mut()
                .map(|s| s as *mut crate::alloc::AllocSink as i64)
                .unwrap_or(0),
        );
        // And one past that, the recursion depth. Compiled recursion runs on
        // the machine stack, which cannot be grown or unwound the way the
        // interpreter's frame budget can, so compiled code counts its own
        // depth and refuses rather than overrunning it.
        marshalled.push(0);

        // Then the safepoint flag, and the sink to park through.
        //
        // The flag is read at every loop back-edge, so it is the address of a
        // plain word rather than anything the poll has to lock. The sink is
        // only touched when that word is non-zero, and it is the *same*
        // pointer the allocation helpers use — one object, so a safe point can
        // prune the allocation list using the frame description without two
        // aliasing pointers to state that must change together.
        if let Some(sink) = sink.as_mut() {
            if !native.reference_slots.is_empty() || !native.array_slots.is_empty() {
                sink.safepoint = Some(crate::alloc::SafepointState {
                    mutators: mutators.clone(),
                    interpreter_roots,
                });
            }
        }
        marshalled.push(mutators.stopping_flag() as i64);
        marshalled.push(
            sink.as_mut()
                .map(|s| s as *mut crate::alloc::AllocSink<'_> as i64)
                .unwrap_or(0),
        );
        marshalled.push(sink.as_ref().map(|s| s.pressure_flag() as i64).unwrap_or(0));
        // The parent frame record. Null here: the frame below a compiled call
        // entered from the interpreter is an *interpreted* one, and its roots
        // come from the closure rather than from a record.
        marshalled.push(0);
        // And the table of callee entries, which `resolve_call_table` filled.
        marshalled.push(
            native
                .call_addresses
                .as_ref()
                .map(|t| t.as_ptr() as i64)
                .unwrap_or(0),
        );

        // SAFETY: `marshalled` holds `arg_count` values plus the trap slot,
        // which is exactly what the emitted code reads. Each array slot points
        // at a descriptor that outlives the call.
        let raw = unsafe { native.call(&marshalled) };

        // Read the trap and hand the buffers back before anything can return.
        // The sink is dropped here too, releasing whatever the call allocated
        // — nothing it allocated can outlive the call, so nothing is lost.
        let trap = marshalled[native.arg_count];
        let returns_value = native.returns_value;
        // A field access reports through the sink rather than the trap slot: a
        // helper has the sink and not the frame. Folded in here so both kinds
        // of failure take the same path out.
        let field_fault = sink.as_ref().map(|s| s.fault).unwrap_or(0);
        drop(sink);
        marshalled.clear();
        descriptors.clear();
        self.scratch_marshalled = marshalled;
        self.scratch_descriptors = descriptors;

        // A failed bounds check cannot throw from compiled code — there is no
        // frame for the interpreter to unwind — so it flags and returns, and
        // the exception is raised here. Declining instead would be wrong:
        // stores that already happened stay done, and re-running the method
        // interpreted would repeat them.
        if field_fault == crate::alloc::FIELD_FAULT_NULL {
            return Some(Err(ExecutionError::null_reference()));
        }
        if field_fault != 0 {
            return Some(Err(ExecutionError::exception(
                ClrExceptionKind::InvalidOperation,
                "compiled code read a field whose shape it did not expect",
            )));
        }

        match trap {
            0 => {}
            1 => {
                return Some(Err(ExecutionError::exception(
                    ClrExceptionKind::IndexOutOfRange,
                    "Index was outside the bounds of the array.",
                )))
            }
            // The heap could not satisfy an allocation. Declining and
            // re-running interpreted would be wrong for the same reason a
            // failed bounds check cannot: stores already made stay made.
            2 => {
                return Some(Err(ExecutionError::exception(
                    ClrExceptionKind::InvalidOperation,
                    "The managed heap could not satisfy an allocation from compiled code.",
                )))
            }
            // Compiled recursion went deeper than the machine stack can be
            // trusted for. The interpreter raises the same exception at its own
            // frame budget, so a program sees one answer either way.
            3 => {
                return Some(Err(ExecutionError::exception(
                    ClrExceptionKind::StackOverflow,
                    "Compiled recursion exceeded the depth this runtime allows.",
                )))
            }
            // A devirtualised call site met a receiver no guard matched.
            //
            // Class-hierarchy analysis said that could not happen, so reaching
            // here means the analysis was wrong rather than the program being
            // unusual — and saying so plainly is the whole point. Reporting it
            // as one of the traps above would send a reader looking for a bug
            // that is not there.
            5 => {
                return Some(Err(ExecutionError::exception(
                    ClrExceptionKind::InvalidOperation,
                    "A devirtualised call met a receiver type its guards did not cover;                      the code generator's hierarchy analysis was wrong.",
                )))
            }
            // A virtual call on a null receiver, which is an ordinary program
            // error rather than anything to do with compilation.
            6 => return Some(Err(ExecutionError::null_reference())),
            // An unknown code means the emitter wrote one this does not know
            // about, which is a bug in this crate. Say that, rather than
            // borrowing whichever message happens to be last.
            other => {
                return Some(Err(ExecutionError::exception(
                    ClrExceptionKind::InvalidOperation,
                    format!("compiled code raised an unrecognised trap ({other})"),
                )))
            }
        }

        if !returns_value {
            return Some(Ok(None));
        }
        let info = loader.registry.method(method);
        Some(Ok(Some(widen(&info.signature.return_type, raw))))
    }

    fn stats(&self) -> (usize, usize) {
        (self.backend.methods_compiled, self.backend.bytes_emitted)
    }
}

impl JitTier {
    /// Whether the method's body contains a backward branch.
    ///
    /// The cheapest signal that a single call could run for a long time, and
    /// the only one available before the call happens. Decoded once per method
    /// and remembered, because this is asked on a hot path.
    fn loops(&mut self, loader: &Loader, method: MethodId) -> bool {
        if let Some(known) = self.loops.get(&method) {
            return *known;
        }
        let answer = match &loader.registry.method(method).kind {
            rustclr_core::MethodKind::Il(body) => {
                match rustclr_core::opcode::decode_all(&body.il) {
                    Ok(instructions) => instructions.iter().any(|ins| {
                        ins.operand.as_target().is_some_and(|t| t <= ins.offset)
                    }),
                    // Undecodable IL is not compilable either; saying "no loop"
                    // leaves the count in charge, and `compile_now` refuses.
                    Err(_) => false,
                }
            }
            _ => false,
        };
        self.loops.insert(method, answer);
        answer
    }

    /// The array types compiled code may allocate.
    ///
    /// `None` when the framework contract is not loaded, which leaves the sink
    /// absent and any `newarr` in compiled code taking the trap path — a
    /// refusal rather than a wrong answer.
    /// Compiles everything `method` can reach and fills in every call table.
    ///
    /// Two passes, and the split is the whole point. A table entry names both
    /// the callee's entry *and the callee's own table*, because a callee reads
    /// its table out of the argument array its caller builds. With one pass and
    /// a cycle — two methods calling each other — the first would have to name
    /// a table the second had not allocated yet, and would name null. So every
    /// reachable method is compiled and its table *allocated* first; only then
    /// are the addresses written, by which point every one of them exists.
    ///
    /// Returns false if anything reachable cannot be compiled, which makes the
    /// caller unusable: it was emitted around calls that would never be filled.
    fn resolve_call_table(&mut self, loader: &Loader, method: MethodId) -> bool {
        if self.resolved.contains(&method) {
            return true;
        }
        let mut reachable = Vec::new();
        if !self.reach(loader, method, &mut reachable) {
            return false;
        }
        // Every table now exists at its final address, so the pairs can name
        // each other however the calls happen to cycle.
        for owner in &reachable {
            let targets = match self.compiled.get(owner) {
                Some(native) => native.call_table.clone(),
                None => return false,
            };
            let mut pairs = Vec::with_capacity(targets.len() * 2);
            for target in &targets {
                let Some(callee) = self.compiled.get(target) else { return false };
                let table = callee
                    .call_addresses
                    .as_ref()
                    .map(|t| t.as_ptr() as i64)
                    .unwrap_or(0);
                pairs.push(callee.entry());
                pairs.push(table);
            }
            // Written *into* the existing allocation, never replaced: another
            // method's table already names this one by address, and a fresh
            // box would leave that pointing at freed memory. That is a jump
            // through a dangling pointer rather than a wrong answer, and it is
            // exactly how the first version of this crashed.
            match self.compiled.get_mut(owner) {
                Some(native) => match native.call_addresses.as_mut() {
                    Some(slots) if slots.len() == pairs.len() => {
                        slots.copy_from_slice(&pairs);
                    }
                    _ => return false,
                },
                None => return false,
            }
            self.resolved.insert(*owner);
        }
        true
    }

    /// Compiles `method` and everything it can reach, allocating each one's
    /// table at its final size so its address is settled.
    fn reach(
        &mut self,
        loader: &Loader,
        method: MethodId,
        found: &mut Vec<MethodId>,
    ) -> bool {
        if !self.compiled.contains_key(&method) && !self.compile_now(loader, method) {
            return false;
        }
        if !self.resolving.insert(method) {
            // Already on the path: a cycle, and its table is already allocated.
            return true;
        }
        let targets = match self.compiled.get(&method) {
            Some(native) => native.call_table.clone(),
            None => {
                self.resolving.remove(&method);
                return false;
            }
        };
        // Allocated before descending, so a cycle coming back round finds an
        // address rather than a hole.
        if let Some(native) = self.compiled.get_mut(&method) {
            if native.call_addresses.is_none() {
                native.call_addresses = Some(vec![0i64; targets.len() * 2].into_boxed_slice());
            }
        }
        found.push(method);
        for target in targets {
            if !self.reach(loader, target, found) {
                self.resolving.remove(&method);
                return false;
            }
        }
        self.resolving.remove(&method);
        true
    }

    fn array_types(&self, loader: &Loader) -> Option<crate::alloc::ArrayTypes> {
        let int_element = loader.registry.find_type_by_name("System.Int32")?;
        let double_element = loader.registry.find_type_by_name("System.Double")?;
        let bool_element = loader.registry.find_type_by_name("System.Boolean")?;
        // `find_sz_array` only knows the constructions something has already
        // named. `System.Double[]` may be absent when nothing has built one
        // yet, so the base `System.Array` stands in — the same fallback
        // `Interpreter::alloc_value_array` uses, and enough for an array whose
        // elements are read by width rather than by type.
        let fallback = loader.core().array;
        Some(crate::alloc::ArrayTypes {
            int_array: loader.registry.find_sz_array(int_element).unwrap_or(fallback),
            int_element,
            double_array: loader.registry.find_sz_array(double_element).unwrap_or(fallback),
            double_element,
            bool_array: loader.registry.find_sz_array(bool_element).unwrap_or(fallback),
            bool_element,
            // Filled in per call from the method about to run: the numbering is
            // that method's own.
            objects: Vec::new(),
            object_fields: Vec::new(),
        })
    }
}

/// An evaluation-stack value as a machine word.
///
/// A `double` goes across as its bit pattern, which is what compiled code
/// expects: floats travel through slots and general-purpose registers as bits
/// there, and only the arithmetic moves them into xmm.
fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::I32(n) => Some(*n as i64),
        Value::I64(n) | Value::NativeInt(n) => Some(*n),
        Value::F(f) => Some(f.to_bits() as i64),
        _ => None,
    }
}

/// Rebuilds the evaluation-stack value a return type calls for.
///
/// The stack type matters: an `int` result must come back as `Value::I32`, or
/// the very next `add` would promote to 64-bit and diverge from the
/// interpreter. Narrow types are truncated exactly as `conv` would.
fn widen(return_type: &TypeSig, raw: i64) -> Value {
    match return_type.unwrap_modifiers() {
        TypeSig::Boolean => Value::I32((raw != 0) as i32),
        TypeSig::I1 => Value::I32(raw as i8 as i32),
        TypeSig::U1 => Value::I32(raw as u8 as i32),
        TypeSig::I2 => Value::I32(raw as i16 as i32),
        TypeSig::U2 | TypeSig::Char => Value::I32(raw as u16 as i32),
        TypeSig::I4 => Value::I32(raw as i32),
        TypeSig::U4 => Value::I32(raw as u32 as i32),
        TypeSig::I8 | TypeSig::U8 => Value::I64(raw),
        // A float result arrives as its bit pattern, and reading it back as an
        // integer was a silent wrong answer of the worst kind: the *bits* were
        // right, so a method returning 1.0 handed back 4.6e18 and every check
        // downstream compared two numbers that were both wrong in the same way.
        TypeSig::R8 => Value::F(f64::from_bits(raw as u64)),
        // `conv.r4` rounds through `f32`; the stack type stays `F`.
        TypeSig::R4 => Value::F(f64::from_bits(raw as u64) as f32 as f64),
        _ => Value::I64(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_narrow_return_type_comes_back_as_int32() {
        // A compiled body leaves 64 bits in rax; the stack type decides how
        // much of it is the answer.
        assert_eq!(widen(&TypeSig::I4, -1), Value::I32(-1));
        assert_eq!(widen(&TypeSig::I8, -1), Value::I64(-1));
        assert_eq!(widen(&TypeSig::Boolean, 5), Value::I32(1));
        assert_eq!(widen(&TypeSig::U1, 0x1FF), Value::I32(0xFF));
        assert_eq!(widen(&TypeSig::I2, 0xFFFF), Value::I32(-1));
    }

    #[test]
    fn numbers_marshal_into_a_register_and_references_do_not() {
        assert_eq!(as_i64(&Value::I32(7)), Some(7));
        assert_eq!(as_i64(&Value::I64(7)), Some(7));
        assert_eq!(as_i64(&Value::Null), None, "a reference is not a machine word");
        // A double crosses as its bit pattern. This used to be `None`, which
        // was right while the backend had no float support and is the wrong
        // answer now.
        assert_eq!(as_i64(&Value::F(1.0)), Some(0x3FF0_0000_0000_0000));
    }

    /// The round trip is what has to hold: whatever `as_i64` sends in, `widen`
    /// must bring back unchanged. Getting one side right and not the other is
    /// how a method returning 1.0 came back as 4.6e18 — correct bits, read as
    /// the wrong kind.
    #[test]
    fn a_double_survives_the_round_trip() {
        for value in [0.0, 1.0, -1.0, 0.25, 2.9289682539682538, f64::MIN, f64::MAX] {
            let raw = as_i64(&Value::F(value)).expect("a double is a machine word");
            assert_eq!(widen(&TypeSig::R8, raw), Value::F(value), "{value} round-trips");
        }
    }

    #[test]
    fn the_default_threshold_leaves_cold_methods_interpreted() {
        let tier = JitTier::new();
        assert!(tier.threshold > 1, "compiling everything on first call defeats tiering");
        assert_eq!(tier.compiled_count(), 0);
    }
}

/// The `{ data pointer, length }` an `int[]` handle stands for.
///
/// `None` when the handle is not an `int[]` — a null reference, or an array of
/// something else. The caller treats that as "cannot marshal", which sends the
/// call back to the interpreter rather than guessing at a layout.
fn int_array_descriptor(heap: &SharedHeap, handle: Handle) -> Option<[i64; 2]> {
    if handle.is_null() {
        return None;
    }
    heap.with::<ClrArray, _>(handle, |array| match &array.storage {
        ArrayStorage::I32(values) => Some([values.as_ptr() as i64, values.len() as i64]),
        // A `double[]` has the same descriptor shape; the compiled code knows
        // the element is eight bytes because the *opcode* said so.
        ArrayStorage::F64(values) => Some([values.as_ptr() as i64, values.len() as i64]),
        ArrayStorage::Bool(values) => Some([values.as_ptr() as i64, values.len() as i64]),
        _ => None,
    })
    .flatten()
}
