//! Allocation from compiled code.
//!
//! The backend used to decline any method that allocates, and the reason was
//! not squeamishness: compiled code reaches no safe point, so a collection
//! running while it executes would have to walk a frame the collector cannot
//! read. It also holds raw pointers into array storage, which a collector that
//! moved objects would invalidate.
//!
//! # Why it is sound now
//!
//! Three facts, and the design needs all three:
//!
//! 1. **`Heap::alloc` does not collect.** It allocates and records pressure. A
//!    collection happens where the runtime asks for one — between two
//!    interpreted instructions, or at a compiled method's loop back-edge —
//!    never underneath an allocation, so an allocation cannot pull the ground
//!    out from under the code that asked for it.
//!
//! 2. **A compiled method cannot return a reference.** Its return type must be
//!    an integer or a float, so everything it allocates is unreachable the
//!    moment the call ends: the sink owns it for the call and drops it after.
//!
//! 3. **Allocation is bounded — by pruning, not by refusal.** A `newarr` inside
//!    a loop was once declined outright, because compiled code never collected
//!    and an unbounded loop would exhaust a heap the interpreter would not. It
//!    is allowed where the method reaches a safe point: [`safepoint`] releases
//!    every allocation the polling frame can no longer name, so memory tracks
//!    what is live rather than what has been allocated. A method that cannot
//!    poll keeps the old rule.
//!
//! The raw pointers this hands to compiled code used to rest on "nothing
//! collects during a call", which is no longer true. They rest on two things
//! that are: this collector never moves an object, and any array the frame can
//! still reach is named by the safe point's map and therefore rooted.
//!
//! When the heap is genuinely full, [`alloc_array`] returns zero, compiled code
//! takes the trap path and the tier raises `OutOfMemoryException`.

use rustclr_core::gc::{Handle, SharedHeap};
use rustclr_core::{ArrayStorage, ClrArray, TypeId};

/// Element kinds compiled code can allocate, as passed in a register.
pub const ELEMENT_I4: i64 = 0;
pub const ELEMENT_R8: i64 = 1;
pub const ELEMENT_U1: i64 = 2;
/// A reference array. Elements are [`Handle`]s, eight bytes each.
///
/// Safe to hand to compiled code because a handle is an opaque token, not an
/// address: it cannot be dereferenced there, and a stale one fails a
/// generation check rather than reading freed memory.
pub const ELEMENT_REF: i64 = 3;

/// The array types a compiled method may allocate, resolved once per call.
#[derive(Clone)]
pub struct ArrayTypes {
    pub int_array: TypeId,
    pub int_element: TypeId,
    pub double_array: TypeId,
    pub double_element: TypeId,
    pub bool_array: TypeId,
    pub bool_element: TypeId,
    /// Classes this method may instantiate, in the order the emitter numbered
    /// them: the type, its instance-field count, and the array type for a
    /// `T[]` of it.
    ///
    /// Baked into the code as a small index rather than looked up by the
    /// helper, so the helper needs no access to the registry and cannot
    /// disagree with what the compiler decided.
    pub objects: Vec<(TypeId, usize, TypeId)>,
    /// The starting value of each field of each class in `objects`.
    ///
    /// `ClrObject::new` fills with `Value::Null`, which is right for a
    /// reference field and wrong for every other kind: a field declared `int`
    /// would read back as a null rather than a zero, and a compiled read would
    /// see a shape it was not expecting. The interpreter computes these once
    /// per type for the same reason; this is the same values, resolved when the
    /// method was compiled.
    pub object_fields: Vec<Vec<rustclr_core::Value>>,
}

/// Arrays a compiled method allocated, alive for exactly one call.
///
/// Compiled code holds the *address* of a descriptor, so a descriptor must not
/// move once handed out — hence the boxes rather than a `Vec<[i64; 2]>` that
/// would reallocate as it grows.
pub struct AllocSink<'a> {
    heap: SharedHeap,
    types: ArrayTypes,
    /// Handles allocated during this call, dropped with the sink.
    ///
    /// Index-aligned with `descriptors`: entry `i` is the array whose
    /// descriptor is `descriptors[i]`.
    ///
    /// These are *not* pinned. Pinning every allocation would keep a compiled
    /// loop's entire history alive for the length of the call — a million
    /// iterations, a million live arrays — which is exactly the unbounded
    /// growth that made allocation in a loop illegal in the first place.
    /// Instead a safe point prunes this list down to what the frame can still
    /// reach, and the collector reclaims the rest.
    allocated: Vec<Handle>,
    /// Descriptors, each at a stable address.
    descriptors: Vec<Box<[i64; 2]>>,
    /// Handles pinned on this call's behalf that the sink did not allocate —
    /// the array arguments the tier marshalled.
    borrowed: Vec<Handle>,
    /// Non-zero when a field access failed; see `FIELD_FAULT_*`.
    ///
    /// Reported through the sink rather than the emitted trap slot because a
    /// helper has the sink and not the frame. The tier reads it after the call
    /// and raises the exception where there is a frame to raise it in.
    pub fault: i64,
    /// Non-zero once an allocation has pushed the heap past its trigger.
    ///
    /// The other half of the poll. The registry's stop flag says *another*
    /// thread wants to collect; this says this call has allocated enough to
    /// want one itself, which with a single thread is the only thing that ever
    /// will. Boxed so its address is stable while compiled code holds it.
    collect_wanted: Box<usize>,
    /// What a safe point in this call needs, when it can reach one.
    ///
    /// Folded in here rather than kept beside it because compiled code would
    /// otherwise hold two pointers to state that must be mutated together —
    /// the allocation list is pruned using the frame description, and taking
    /// `&mut` to both through separate pointers is exactly the aliasing this
    /// runtime exists to avoid.
    pub safepoint: Option<SafepointState<'a>>,
}

/// The frame description and registry a safe point needs.
pub struct SafepointState<'a> {
    /// The registry to park in.
    pub mutators: rustclr_core::gc::safepoint::Mutators,
    /// Roots of the interpreter frames below this call.
    ///
    /// A closure rather than a list because a collection is rare and walking
    /// those frames is not free: it is only called when one is happening.
    pub interpreter_roots: &'a mut dyn FnMut() -> Vec<Handle>,
}

/// Which slots of one compiled method's frame hold what.
///
/// Per *method*, not per call, and owned by the compiled method rather than by
/// the call: once a compiled method can call another compiled method, the
/// frame that polls is not necessarily the frame that was entered, and a single
/// description shared by the call would be read against the wrong frame.
///
/// `repr(C)` because compiled code holds its address and the helper reads it
/// back — the same reasoning that makes `Handle` `repr(transparent)`. It is a
/// plain record of pointers and lengths with nothing to reorder.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct FrameMap {
    /// Byte offsets from the frame pointer of slots holding a **handle**.
    pub reference_slots: *const i32,
    pub reference_len: usize,
    /// Byte offsets of slots holding an **array descriptor address**.
    ///
    /// Separate because they are read differently: a reference slot *is* a
    /// handle, while a descriptor address has to be matched back to the
    /// allocation that produced it.
    pub array_slots: *const i32,
    pub array_len: usize,
}

// SAFETY: the pointers are into buffers the owning `NativeMethod` keeps alive
// for as long as its code can run, and nothing writes through them.
unsafe impl Send for FrameMap {}
unsafe impl Sync for FrameMap {}

/// What one compiled frame tells a safe point about itself.
///
/// Compiled code writes this into its own frame on entry and hands its address
/// to anything it calls, so the frames form a chain a collection can walk. That
/// chain is the whole reason a compiled method may call another one: without
/// it, a callee that polls would describe its own frame and the caller's live
/// references — sitting in a frame nothing named — would be swept.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FrameRecord {
    /// The record of the compiled frame that called this one, or null.
    pub parent: *const FrameRecord,
    /// This frame's base pointer, which the offsets in `map` are relative to.
    pub frame: i64,
    /// What this frame's slots hold.
    pub map: *const FrameMap,
}

impl<'a> AllocSink<'a> {
    pub fn new(heap: SharedHeap, types: ArrayTypes) -> Self {
        Self {
            heap,
            types,
            allocated: Vec::new(),
            descriptors: Vec::new(),
            borrowed: Vec::new(),
            fault: 0,
            collect_wanted: Box::new(0),
            safepoint: None,
        }
    }

    /// Pins an object the caller handed in, for the duration of this call.
    ///
    /// The tier calls this for every array argument: compiled code receives a
    /// pointer into the array's storage, and nothing else roots that array
    /// while the call runs — the interpreter did not push a frame for it.
    pub fn borrow(&mut self, handle: Handle) {
        if self.heap.pin(handle) {
            self.borrowed.push(handle);
        }
    }

    /// Raises the pressure flag if the heap has had enough.
    ///
    /// Asked here because this is already inside the heap's lock — the poll at
    /// the next back-edge only has to read a word.
    fn note_pressure(&mut self) {
        if self.heap.should_collect() {
            *self.collect_wanted = 1;
        }
    }

    /// The word compiled code polls to learn this call wants a collection.
    pub fn pressure_flag(&self) -> *const usize {
        self.collect_wanted.as_ref()
    }

    /// Drops every allocation this call can no longer reach.
    ///
    /// `live` holds the descriptor addresses and handles still named by the
    /// polling frame's slots. Anything else was allocated earlier in the loop
    /// and abandoned, so letting go of it here is what keeps a compiled loop's
    /// memory use flat instead of proportional to its trip count.
    ///
    /// Returns the handles that survived, which are the roots the frame
    /// contributes.
    pub fn prune(&mut self, live: &[i64]) -> Vec<Handle> {
        let mut kept_handles = Vec::new();
        let mut kept_descriptors = Vec::new();
        let mut survivors = Vec::new();
        for (handle, descriptor) in
            core::mem::take(&mut self.allocated).into_iter().zip(core::mem::take(&mut self.descriptors))
        {
            let address = descriptor.as_ref().as_ptr() as i64;
            let reachable =
                live.contains(&address) || live.contains(&(handle.to_bits() as i64));
            if reachable {
                survivors.push(handle);
                kept_handles.push(handle);
                kept_descriptors.push(descriptor);
            }
        }
        self.allocated = kept_handles;
        self.descriptors = kept_descriptors;
        survivors.extend(self.borrowed.iter().copied());
        survivors
    }

    /// How many arrays this call allocated, for tests and diagnostics.
    pub fn allocated(&self) -> usize {
        self.allocated.len()
    }

    /// Allocates one instance of the class the emitter numbered `shape`.
    fn make_object(&mut self, shape: usize) -> Option<i64> {
        let (type_id, count, _) = *self.types.objects.get(shape)?;
        let fields = match self.types.object_fields.get(shape) {
            Some(defaults) if defaults.len() == count => defaults.clone(),
            // No defaults resolved for this shape, which means the compiler and
            // this helper disagree about it. Refusing beats handing back an
            // object whose fields read as the wrong kind.
            _ => return None,
        };
        let handle = self
            .heap
            .try_alloc(rustclr_core::ClrObject { type_id, fields, monitor: None })?;
        self.allocated.push(handle);
        // An object has no descriptor, but the two lists stay index-aligned so
        // pruning can walk them together.
        self.descriptors.push(Box::new([0, 0]));
        self.note_pressure();
        Some(handle.to_bits() as i64)
    }

    fn make(&mut self, kind: i64, length: usize) -> Option<i64> {
        let (array_type, element_type, storage) = match kind {
            ELEMENT_I4 => (
                self.types.int_array,
                self.types.int_element,
                ArrayStorage::I32(vec![0; length]),
            ),
            ELEMENT_R8 => (
                self.types.double_array,
                self.types.double_element,
                ArrayStorage::F64(vec![0.0; length]),
            ),
            ELEMENT_U1 => (
                self.types.bool_array,
                self.types.bool_element,
                ArrayStorage::Bool(vec![false; length]),
            ),
            // A reference array. `kind` above `ELEMENT_REF` names which class
            // the elements are, by the same index `alloc_object` takes.
            k if k >= ELEMENT_REF => {
                let (element, _, array) = *self.types.objects.get((k - ELEMENT_REF) as usize)?;
                (array, element, ArrayStorage::Refs(vec![Handle::NULL; length]))
            }
            // The emitter only ever passes the kinds above; anything else
            // means the compiled code and this helper disagree, which is a bug
            // rather than a program error.
            _ => return None,
        };

        let handle = self.heap.try_alloc(ClrArray {
            array_type,
            element_type,
            storage,
            dimensions: vec![length as u32],
        })?;
        self.allocated.push(handle);

        // The descriptor compiled code reads: data pointer, then length.
        let descriptor = self
            .heap
            .with::<ClrArray, _>(handle, |array| match &array.storage {
                ArrayStorage::I32(v) => [v.as_ptr() as i64, v.len() as i64],
                ArrayStorage::F64(v) => [v.as_ptr() as i64, v.len() as i64],
                // `Vec<bool>` is one byte per element with the same layout a
                // byte buffer has, which is exactly what compiled code reads.
                ArrayStorage::Bool(v) => [v.as_ptr() as i64, v.len() as i64],
                // `Handle` is `repr(transparent)` over `u64`, so this buffer is
                // exactly a run of 64-bit tokens — which is what compiled code
                // reads and writes.
                ArrayStorage::Refs(v) => [v.as_ptr() as i64, v.len() as i64],
                _ => [0, 0],
            })?;
        if descriptor[0] == 0 {
            return None;
        }
        self.descriptors.push(Box::new(descriptor));
        self.note_pressure();
        Some(self.descriptors.last()?.as_ref().as_ptr() as i64)
    }
}

impl Drop for AllocSink<'_> {
    /// Releases every pin this call took.
    ///
    /// The sink lives for exactly one compiled call, so this is where "for the
    /// duration of the call" is actually enforced. Anything it allocated is
    /// unreachable once the call returns — a compiled method cannot return a
    /// reference — and anything it borrowed is the caller's to keep.
    fn drop(&mut self) {
        // Only the borrowed handles were pinned; what this call allocated is
        // unreachable the moment it returns, because a compiled method cannot
        // return a reference.
        for handle in &self.borrowed {
            self.heap.unpin(*handle);
        }
    }
}

/// Allocates a class instance for compiled code, returning its handle's bits.
///
/// Returns **zero** — `Handle::NULL`'s representation — when the heap could not
/// satisfy the request, which compiled code branches on exactly as it does for
/// a failed array allocation.
///
/// The constructor is *not* run. Only types whose constructor does nothing but
/// chain to `System.Object`'s reach here, which the compiler checks before it
/// emits the call; every field starts at its zero, which is what that
/// constructor would have left them at anyway.
///
/// # Safety
///
/// As [`alloc_array`]: `sink` is the pointer the tier placed in the argument
/// array, and the call must still be running.
pub extern "C" fn alloc_object(sink: *mut AllocSink<'_>, shape: i64) -> i64 {
    if sink.is_null() || shape < 0 {
        return 0;
    }
    // SAFETY: see `alloc_array`.
    let sink = unsafe { &mut *sink };
    sink.make_object(shape as usize).unwrap_or(0)
}

/// The runtime type of an object, as a registry index.
///
/// This is what makes guarded devirtualisation possible without compiled code
/// knowing any Rust layout: the comparison happens against a number the
/// compiler baked in, and the lookup that produces it happens here, in Rust.
///
/// Returns **-1** for a null receiver, which compiled code turns into the
/// `NullReferenceException` a virtual call on null owes the program, and
/// **-2** for a handle that does not resolve to a class instance. The two are
/// told apart because they mean different things: the first is an ordinary
/// program error, the second can only be a bug in this runtime.
///
/// # Safety
///
/// As [`alloc_array`].
pub extern "C" fn type_of(sink: *mut AllocSink<'_>, handle: i64) -> i64 {
    if sink.is_null() {
        return -1;
    }
    // SAFETY: see `alloc_array`.
    let sink = unsafe { &mut *sink };
    let handle = Handle::from_bits(handle as u64);
    if handle.is_null() {
        return -1;
    }
    sink.heap
        .with::<rustclr_core::ClrObject, _>(handle, |o| o.type_id.index() as i64)
        .unwrap_or(-2)
}

/// Allocates an array for compiled code.
///
/// Returns the address of a two-word descriptor, or **zero** when the heap
/// could not satisfy the request — compiled code branches on that and takes
/// the trap path, exactly as a failed bounds check does.
///
/// # Safety
///
/// `sink` must be the pointer the tier placed in the argument array for this
/// call, and the call must still be running. Compiled code reads it out of a
/// slot the tier wrote and never stores it anywhere, so it cannot outlive the
/// sink.
pub extern "C" fn alloc_array(sink: *mut AllocSink<'_>, kind: i64, length: i64) -> i64 {
    if sink.is_null() || length < 0 {
        return 0;
    }
    // SAFETY: the tier wrote this pointer into the argument slot compiled code
    // read it from, and owns the sink for the duration of the call.
    let sink = unsafe { &mut *sink };
    sink.make(kind, length as usize).unwrap_or(0)
}

/// A safe point reached by compiled code.
///
/// Publishes the roots of *every* compiled frame in the chain, releases what
/// they can no longer reach, and parks until the collection is over. The object
/// graph does not move — this collector never relocates — so nothing has to be
/// written back, and that is the property that makes a safe point in compiled
/// code a matter of describing frames rather than rewriting them.
///
/// Walking the chain rather than describing one frame is what makes a compiled
/// method safe to call from another one. The callee polls; the caller's live
/// references are in a frame the callee knows nothing about, and would be swept
/// if the walk stopped at the top.
///
/// # Safety
///
/// `sink` must be the pointer the tier placed in the argument array and the
/// call must still be running. `record` must be the polling frame's own record,
/// whose chain is live for the same reason its code is.
pub extern "C" fn safepoint(sink: *mut AllocSink<'_>, record: *const FrameRecord) {
    if sink.is_null() || record.is_null() {
        return;
    }
    // SAFETY: the tier owns the sink for the duration of the call, and
    // compiled code reads the pointer out of the slot the tier wrote.
    let sink = unsafe { &mut *sink };
    let Some(mut state) = sink.safepoint.take() else { return };

    let mut live: Vec<i64> = Vec::new();
    let mut handles: Vec<Handle> = Vec::new();
    let mut current = record;
    // Bounded: a chain longer than the depth limit compiled code enforces
    // means the chain itself is corrupt, and looping forever on it would turn
    // a bug into a hang.
    for _ in 0..MAX_FRAME_CHAIN {
        if current.is_null() {
            break;
        }
        // SAFETY: each record was written by the prologue of a frame that is
        // still executing, and its parent by the frame that called it.
        let frame = unsafe { &*current };
        if frame.map.is_null() || frame.frame == 0 {
            break;
        }
        let map = unsafe { &*frame.map };
        let read = |offset: i32| -> i64 {
            let address = (frame.frame + offset as i64) as *const i64;
            unsafe { address.read() }
        };
        if !map.array_slots.is_null() {
            let slots = unsafe { core::slice::from_raw_parts(map.array_slots, map.array_len) };
            live.extend(slots.iter().map(|o| read(*o)));
        }
        if !map.reference_slots.is_null() {
            let slots =
                unsafe { core::slice::from_raw_parts(map.reference_slots, map.reference_len) };
            handles.extend(
                slots
                    .iter()
                    .map(|o| Handle::from_bits(read(*o) as u64))
                    .filter(|h| !h.is_null()),
            );
        }
        current = frame.parent;
    }
    live.extend(handles.iter().map(|h| h.to_bits() as i64));

    // Everything these frames allocated and can no longer reach is released
    // here, which is what keeps a loop's memory flat rather than proportional
    // to its trip count.
    let mut roots = sink.prune(&live);
    roots.extend(handles);

    let mutators = state.mutators.clone();
    let heap = sink.heap.clone();
    let below = &mut state.interpreter_roots;

    // Park if another thread has asked to collect.
    mutators.poll(|| {
        let mut all = below();
        all.extend(roots.iter().copied());
        all
    });

    // And collect on this thread's own behalf if the heap has earned it.
    //
    // Both halves are needed and they do different jobs. The park above is
    // what lets *another* thread finish a collection while this one is deep in
    // a compiled loop — without it a long compiled loop stops the world
    // indefinitely. This second half is what makes a compiled loop safe in a
    // program with only one thread: nothing else will ever set the flag, so
    // without it a loop could allocate without bound and never reclaim
    // anything, which is precisely the failure the old "no allocation inside a
    // loop" rule existed to prevent.
    if heap.should_collect() {
        let mut all = below();
        all.extend(roots.iter().copied());
        mutators.stop_the_world(all, |reachable| heap.collect(reachable));
    }
    *sink.collect_wanted = 0;
    sink.safepoint = Some(state);
}

/// How many compiled frames a safe point will walk before giving up.
///
/// Comfortably above the depth compiled recursion is allowed to reach, so this
/// only ever fires on a chain that is already corrupt.
const MAX_FRAME_CHAIN: usize = 8192;

/// How a field's value crosses between compiled code and the heap.
///
/// Compiled code holds one 64-bit word per value and does not know what shape
/// the heap keeps it in — that is the whole point. This says how to convert,
/// and it is baked into the emitted call from the field's *declared* type, so
/// the compiler and the helper cannot disagree about it.
pub const FIELD_I4: i64 = 0;
pub const FIELD_I8: i64 = 1;
pub const FIELD_R8: i64 = 2;
pub const FIELD_REF: i64 = 3;

/// Reads instance field `slot` of `receiver`.
///
/// Returns the value as a 64-bit word — an integer, a double's bit pattern, or
/// a handle's bits. **This is the alternative to putting `Value`'s memory
/// layout into the code generator's ABI**, which this runtime refuses: a Rust
/// enum with boxed payloads has no layout the language promises, and emitting
/// machine code that read one would fail as a wrong number rather than a crash
/// the day a compiler reordered the variants. A call costs more than a load
/// and cannot be wrong in that way.
///
/// A null receiver, a stale handle or an out-of-range slot all raise the
/// trap flag through `sink` rather than returning a plausible zero.
///
/// # Safety
///
/// As [`alloc_array`]: `sink` is the pointer the tier placed in the argument
/// array and the call must still be running.
pub extern "C" fn field_load(sink: *mut AllocSink<'_>, receiver: i64, slot: i64, kind: i64) -> i64 {
    if sink.is_null() {
        return 0;
    }
    // SAFETY: see `alloc_array`.
    let sink = unsafe { &mut *sink };
    let handle = Handle::from_bits(receiver as u64);
    if handle.is_null() || slot < 0 {
        sink.fault = FIELD_FAULT_NULL;
        return 0;
    }
    let value = sink
        .heap
        .with::<rustclr_core::ClrObject, _>(handle, |o| o.fields.get(slot as usize).cloned())
        .flatten();
    // The kind is checked, not merely used to convert. A reference stored as a
    // number would round-trip perfectly through compiled code and be invisible
    // to the collector, which traces `Value::Obj` and not `Value::I64` — so the
    // chain under it would be swept while compiled code still walked it. That
    // is a wrong answer arriving much later than the mistake, which is exactly
    // the failure mode this whole helper exists to avoid, so the load refuses a
    // shape that does not match what the field was declared to hold.
    match (value, kind) {
        (Some(rustclr_core::Value::I32(v)), FIELD_I4) => v as i64,
        (Some(rustclr_core::Value::I32(v)), FIELD_I8) => v as i64,
        (Some(rustclr_core::Value::I64(v)), FIELD_I8)
        | (Some(rustclr_core::Value::NativeInt(v)), FIELD_I8) => v,
        (Some(rustclr_core::Value::F(v)), FIELD_R8) => v.to_bits() as i64,
        (Some(rustclr_core::Value::Obj(h)), FIELD_REF) => h.to_bits() as i64,
        (Some(rustclr_core::Value::Null), FIELD_REF) => 0,
        // A shape the compiled call did not expect. Refusing beats guessing:
        // the tier turns this into an exception where there is a frame to
        // raise it in.
        _ => {
            sink.fault = FIELD_FAULT_SHAPE;
            0
        }
    }
}

/// Writes instance field `slot` of `receiver`.
///
/// The kind comes from the field's declared type rather than from what the
/// slot currently holds: a reference field starts as `Value::Null`, and
/// deciding from that would make the first store to it write an integer.
///
/// # Safety
///
/// As [`field_load`].
pub extern "C" fn field_store(
    sink: *mut AllocSink<'_>,
    receiver: i64,
    slot: i64,
    kind: i64,
    value: i64,
) {
    if sink.is_null() {
        return;
    }
    // SAFETY: see `alloc_array`.
    let sink = unsafe { &mut *sink };
    let handle = Handle::from_bits(receiver as u64);
    if handle.is_null() || slot < 0 {
        sink.fault = FIELD_FAULT_NULL;
        return;
    }
    let stored = match kind {
        FIELD_I8 => rustclr_core::Value::I64(value),
        FIELD_R8 => rustclr_core::Value::F(f64::from_bits(value as u64)),
        FIELD_REF => {
            let target = Handle::from_bits(value as u64);
            if target.is_null() {
                rustclr_core::Value::Null
            } else {
                rustclr_core::Value::Obj(target)
            }
        }
        // Narrowing is the compiler's job on the way in; a field declared
        // `int` holds an `I32` however wide the register was.
        _ => rustclr_core::Value::I32(value as i32),
    };
    let wrote = sink
        .heap
        .with_mut::<rustclr_core::ClrObject, _>(handle, |o| match o.fields.get_mut(slot as usize) {
            Some(field) => {
                *field = stored;
                true
            }
            None => false,
        })
        .unwrap_or(false);
    if !wrote {
        sink.fault = FIELD_FAULT_SHAPE;
    }
}

/// A field access failed on a null or stale receiver.
pub const FIELD_FAULT_NULL: i64 = 1;
/// A field held something the compiled call was not expecting.
pub const FIELD_FAULT_SHAPE: i64 = 2;
