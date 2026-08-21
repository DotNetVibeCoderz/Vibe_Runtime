//! The IL execution engine.
//!
//! The loop is *iterative*: managed calls push onto an explicit [`Frame`]
//! vector rather than recursing into `execute`. Deeply recursive managed code
//! therefore exhausts a configurable frame budget and throws
//! `StackOverflowException` instead of aborting the process by blowing the
//! native stack.
//!
//! Each method is decoded once into a [`CompiledMethod`] and cached. That
//! prepass is the seam a real JIT plugs into: `rustclr-jit` can swap the
//! interpreted body for native code behind the same interface.

use crate::error::{ClrExceptionKind, ExecResult, ExecutionError};
use crate::host::Host;
#[cfg(feature = "std")]
use crate::host::SystemHost;
use crate::loader::Loader;
use crate::objects::*;
use crate::opcode::{decode_all, Instruction, Op, Operand};
use crate::types::*;
use crate::value::{ByRef, StructValue, Value};
use rustclr_gc::safepoint::{Mutators, Registration};
use rustclr_gc::{Handle, SharedHeap};
use rustclr_metadata::{ExceptionClause, HandlerKind, Token, TypeSig};

#[allow(unused_imports)]
use crate::prelude::*;


/// A method decoded and prepared for execution.
#[derive(Debug)]
pub struct CompiledMethod {
    pub method: MethodId,
    pub instructions: Vec<Instruction>,
    /// IL offset to instruction index; branch targets are IL offsets.
    pub index_of_offset: HashMap<u32, usize>,
    pub max_stack: u16,
    pub local_types: Vec<TypeSig>,
    pub exception_clauses: Vec<ExceptionClause>,
    pub init_locals: bool,
    pub arg_count: usize,
}

impl CompiledMethod {
    fn index_for(&self, offset: u32) -> ExecResult<usize> {
        self.index_of_offset.get(&offset).copied().ok_or_else(|| {
            ExecutionError::InvalidProgram(format!("branch to invalid IL offset {offset:#x}"))
        })
    }
}

/// One activation record.
pub struct Frame {
    pub method: MethodId,
    pub assembly: AssemblyId,
    /// Identity used by managed pointers into this frame.
    pub id: u32,
    code: Arc<CompiledMethod>,
    pub args: Vec<Value>,
    pub locals: Vec<Value>,
    pub stack: Vec<Value>,
    /// Index into `code.instructions`.
    pc: usize,
    /// IL offsets of finally handlers still to run, in execution order.
    pending_finallies: Vec<u32>,
    /// Where control resumes once every queued finally has run.
    /// [`PROPAGATE`] means "continue unwinding the in-flight exception".
    finally_resume: Option<u32>,
    /// Exception being propagated while those finally blocks run.
    in_flight: Option<Box<ExecutionError>>,
    /// `constrained.` prefix token awaiting the next `callvirt`.
    constrained: Option<Token>,
    /// The closed construction this call was made *through*, when the call site
    /// named one.
    ///
    /// Only a static method on a generic type needs it. An instance method asks
    /// its receiver — `this` is a `Tally<int>` or a `Tally<string>` and those
    /// are different runtime types — but a static method has no receiver, and
    /// the body is shared by every construction. The call site is the one place
    /// that still knows: `call Tally<int>::Add()` is a `MemberRef` whose owner
    /// is the construction, not the definition.
    pub construction: Option<TypeId>,
    /// Object under construction, pushed to the caller when a `.ctor` returns.
    pending_newobj: Option<Handle>,
    /// Set when `pending_newobj` is a one-field cell holding a value type
    /// rather than the instance itself: the caller wants the field, not the box.
    pending_newobj_is_cell: bool,
    /// True for a frame created to evaluate an exception filter.
    ///
    /// It shares the method and code of the frame being unwound, so without
    /// this there is nothing to distinguish the two — and `endfilter` needs to
    /// know it is ending a filter rather than falling through one during
    /// ordinary flow.
    pub(crate) is_filter: bool,
}

/// Sentinel `finally_resume` meaning "resume unwinding" rather than "branch".
pub(crate) const PROPAGATE: u32 = u32::MAX;

impl Frame {
    fn current_offset(&self) -> u32 {
        self.code
            .instructions
            .get(self.pc)
            .map(|i| i.offset)
            .unwrap_or_else(|| self.code.instructions.last().map_or(0, |i| i.next_offset()))
    }

    /// Offset of the instruction that is currently executing (pc has already
    /// been advanced past it).
    fn executing_offset(&self) -> u32 {
        let index = self.pc.saturating_sub(1);
        self.code.instructions.get(index).map_or(0, |i| i.offset)
    }
}

/// A code generator the interpreter can hand methods to.
///
/// The interpreter cannot depend on `rustclr-jit` — that crate depends on this
/// one — so the seam is a trait here and the backend is installed by whoever
/// builds the runtime. Declining is the normal answer: a backend that handles
/// only some method shapes is useful immediately, and everything it turns down
/// is interpreted exactly as before.
pub trait NativeTier: Send {
    fn name(&self) -> &'static str;

    /// Offers a call to the backend.
    ///
    /// `None` means "not compiled — interpret it". `Some(Ok(result))` means the
    /// method ran natively; the inner `Option` is its return value, absent for
    /// a void method. `Some(Err(..))` means it ran and *faulted* — an array
    /// index out of range, say — which is not the same as declining: the method
    /// has already had its effects and must not be run again.
    ///
    /// `heap` is here because a backend may need to look through a handle to
    /// the storage behind it. It is a [`SharedHeap`], so a backend reads it
    /// through a closure and holds no borrow of its own; it reads to marshal
    /// arguments and never allocates, which is the property that makes it safe
    /// to hold a pointer into an object for the duration of a compiled call.
    fn try_execute(
        &mut self,
        loader: &Loader,
        heap: &SharedHeap,
        method: MethodId,
        args: &[Value],
    ) -> Option<ExecResult<Option<Value>>>;

    /// Methods compiled and bytes emitted, for `--stats`.
    fn stats(&self) -> (usize, usize);
}

/// A natively implemented method.
///
/// Receives the interpreter — for heap access and re-entrant managed calls —
/// and the argument list including `this`. Returns the value to push, or `None`
/// for a void method.
pub type NativeFn = fn(&mut Interpreter, &[Value]) -> ExecResult<Option<Value>>;

/// Execution limits, so a runaway program fails predictably.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_frames: usize,
    /// `None` means unlimited.
    pub max_instructions: Option<u64>,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_frames: 4096, max_instructions: None }
    }
}

/// Counters surfaced to the profiler and the CLI.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExecutionStats {
    pub instructions_executed: u64,
    pub calls: u64,
    pub native_calls: u64,
    pub allocations: u64,
    pub max_frame_depth: usize,
    pub collections: u64,
    /// Calls that ran as compiled machine code rather than being interpreted.
    pub native_tier_calls: u64,
}

/// Outcome of entering a method.
enum Entered {
    /// A managed frame was pushed; the caller should run the loop.
    Frame,
    /// A native method ran to completion, producing this value.
    Native(Option<Value>),
}

/// What one instruction did to control flow.
enum StepOutcome {
    Continue,
    /// The bottom frame of the current run returned this value.
    Returned(Option<Value>),
}

/// The execution engine.
pub struct Interpreter {
    pub loader: Loader,
    /// The managed heap.
    ///
    /// Shared rather than owned: every accessor keeps its borrow inside a
    /// closure, so the heap can sit behind a lock without any call site
    /// noticing. That is the prerequisite for more than one thread running
    /// managed code — see `rustclr_gc::shared` and `rustclr_gc::safepoint`.
    pub heap: SharedHeap,
    /// The threads collecting against `heap`. One, unless something spawned.
    mutators: Mutators,
    /// This interpreter's membership in `mutators`, dropped with it.
    ///
    /// An interpreter *is* a mutator: it is the thing that holds references
    /// into the heap and the thing that can describe its own roots. Registering
    /// per-interpreter rather than per-thread keeps those two facts together.
    _registration: Registration,
    pub host: Box<dyn Host>,
    pub limits: Limits,
    pub stats: ExecutionStats,
    natives: HashMap<String, NativeFn>,
    /// Optional native code generator. `None` means everything is interpreted.
    pub native_tier: Option<Box<dyn NativeTier>>,
    /// Frame depth below which a `ret` is returning out of the current
    /// invocation rather than into a managed caller. Zero at the top level;
    /// raised while a native method calls back into managed code.
    frame_floor: usize,
    code_cache: HashMap<MethodId, Arc<CompiledMethod>>,
    frames: Vec<Frame>,
    next_frame_id: u32,
    /// `ldstr` results, keyed by assembly index and `#US` offset.
    literal_cache: HashMap<(u32, u32), Handle>,
    /// Interned strings, which are also GC roots.
    interned: HashMap<String, Handle>,
    /// One `System.Type` instance per runtime type, so `typeof(T) == typeof(T)`
    /// compares equal by reference the way .NET guarantees. Also GC roots.
    type_objects: HashMap<TypeId, Handle>,
    exit_requested: Option<i32>,
    /// The monitors of this runtime — what `lock (x)` takes.
    monitors: crate::monitor::Monitors,
    /// The threads that run `Task.Run` and `Parallel.*`, started on first use.
    ///
    /// Lazily, because building it copies the loader once per core, and a
    /// program that never starts a task should not pay for that.
    #[cfg(feature = "std")]
    pool: Option<crate::pool::TaskPool>,
    /// Threads started by this one, waiting to be joined.
    #[cfg(feature = "std")]
    threads: HashMap<u64, std::thread::JoinHandle<(ExecResult<()>, bool)>>,
    #[cfg(feature = "std")]
    next_thread_id: u64,
    /// Method count when this interpreter was made a worker, or zero.
    ///
    /// The tripwire for [`Interpreter::diverged`].
    loaded_size: usize,
    /// Type arguments the call site named for the native being serviced.
    ///
    /// A framework generic is one runtime type for every construction, so the
    /// method cannot be asked what `T` is. The *reference* that reached it can:
    /// `new Span<int>(ptr, 4)` spells `int` out in its `TypeSpec`. Set for the
    /// duration of one native call, like `current_native`.
    current_native_type_args: Vec<TypeId>,
    /// Staged by the call site, taken by the next non-IL call.
    pending_type_args: Vec<TypeId>,
    /// The method a native handler is currently servicing.
    ///
    /// Native methods are plain `fn` pointers with no user data, so a handler
    /// that needs to know *which* declaration invoked it — the P/Invoke bridge,
    /// above all — reads it from here.
    current_native: Option<MethodId>,
}

#[cfg(feature = "std")]
impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    /// An interpreter on real stdio and the system clock.
    ///
    /// Needs `std`, because [`SystemHost`] does. Without it, construct one with
    /// [`Interpreter::with_host`] and a host of your own — which is what the
    /// board firmwares do.
    #[cfg(feature = "std")]
    pub fn new() -> Self {
        Self::with_host(Box::new(SystemHost::new()))
    }

    pub fn with_host(host: Box<dyn Host>) -> Self {
        let mutators = Mutators::default();
        Self {
            loader: Loader::new(),
            heap: SharedHeap::default(),
            mutators: mutators.clone(),
            _registration: mutators.register(),
            host,
            limits: Limits::default(),
            stats: ExecutionStats::default(),
            natives: HashMap::new(),
            native_tier: None,
            frame_floor: 0,
            code_cache: HashMap::new(),
            frames: Vec::new(),
            next_frame_id: 1,
            literal_cache: HashMap::new(),
            interned: HashMap::new(),
            type_objects: HashMap::new(),
            exit_requested: None,
            current_native: None,
            monitors: crate::monitor::Monitors::default(),
            #[cfg(feature = "std")]
            pool: None,
            #[cfg(feature = "std")]
            threads: HashMap::new(),
            #[cfg(feature = "std")]
            next_thread_id: 0,
            loaded_size: 0,
            current_native_type_args: Vec::new(),
            pending_type_args: Vec::new(),
        }
    }

    /// Registers a native implementation under a binding key.
    pub fn register_native(&mut self, key: impl Into<String>, f: NativeFn) {
        self.natives.insert(key.into(), f);
    }

    pub fn native_count(&self) -> usize {
        self.natives.len()
    }

    pub fn has_native(&self, key: &str) -> bool {
        self.natives.contains_key(key)
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_requested
    }

    pub fn request_exit(&mut self, code: i32) {
        self.exit_requested = Some(code);
    }

    pub fn frame_depth(&self) -> usize {
        self.frames.len()
    }

    /// The method whose native handler is running, if any.
    pub fn current_native_method(&self) -> Option<MethodId> {
        self.current_native
    }

    /// Type arguments the call site named, for a native on a framework generic.
    ///
    /// Empty when the call site named none — an ordinary method, or a
    /// construction this runtime gave a runtime type of its own, where the
    /// method already knows.
    pub fn current_type_arguments(&self) -> &[TypeId] {
        &self.current_native_type_args
    }

    /// Assembly of the currently executing frame.
    ///
    /// Native methods need this to interpret a metadata token argument, which
    /// is only meaningful relative to the assembly that emitted it.
    pub fn current_assembly(&self) -> Option<AssemblyId> {
        self.frames.last().map(|f| f.assembly)
    }

    /// Output captured by the host, when it buffers rather than streams.
    pub fn captured_output(&self) -> Option<String> {
        self.host.captured_output().map(str::to_string)
    }

    /// Dereferences a managed pointer. Native methods need this because C#
    /// passes `this` for a value type as a `&`.
    pub fn load_indirect_public(&mut self, r: ByRef) -> ExecResult<Value> {
        self.load_indirect(r)
    }

    /// Writes through a managed pointer, for native methods with `out`/`ref`
    /// parameters such as `int.TryParse`.
    pub fn store_indirect_public(&mut self, r: ByRef, value: Value) -> ExecResult<()> {
        self.store_indirect(r, value)
    }

    /// A textual stack trace of the current frames, innermost first.
    pub fn stack_trace(&self) -> Vec<String> {
        self.frames
            .iter()
            .rev()
            .map(|f| {
                let m = self.loader.registry.method(f.method);
                let t = self.loader.registry.ty(m.declaring_type).full_name();
                format!("   at {}.{} (IL_{:04X})", t, m.name, f.current_offset())
            })
            .collect()
    }

    // -- heap helpers --------------------------------------------------------

    pub fn alloc_string(&mut self, s: &str) -> Handle {
        self.stats.allocations += 1;
        self.heap.alloc(ClrString::from_str(s))
    }

    pub fn alloc_clr_string(&mut self, s: ClrString) -> Handle {
        self.stats.allocations += 1;
        self.heap.alloc(s)
    }

    pub fn string_value(&self, handle: Handle) -> Option<String> {
        self.heap.with::<ClrString, _>(handle, |s| s.to_rust_string())
    }

    /// Reads a value expected to be a string. `null` yields `None`.
    pub fn value_as_string(&self, v: &Value) -> Option<String> {
        match v {
            Value::Obj(h) => self.string_value(*h),
            _ => None,
        }
    }

    /// Interns a string, returning the shared instance.
    pub fn intern(&mut self, s: &str) -> Handle {
        if let Some(h) = self.interned.get(s) {
            if self.heap.is_valid(*h) {
                return *h;
            }
        }
        let h = self.alloc_string(s);
        self.interned.insert(s.to_string(), h);
        h
    }

    /// Allocates an instance of a reference type with fields at their defaults.
    pub fn alloc_object(&mut self, type_id: TypeId) -> Handle {
        let fields = self.instance_fields(type_id);
        let mut obj = ClrObject::new(type_id, fields.len());
        for (i, field) in fields.iter().enumerate() {
            let sig = self.loader.registry.field(*field).signature.clone();
            obj.fields[i] = self.default_value_for(&sig);
        }
        self.stats.allocations += 1;
        self.heap.alloc(obj)
    }

    /// Instance fields of a type, base-first so slots are stable in subclasses.
    pub fn instance_fields(&self, type_id: TypeId) -> Vec<FieldId> {
        let mut chain: Vec<TypeId> = self.loader.registry.base_chain(type_id).collect();
        chain.reverse();
        let mut out = Vec::new();
        for t in chain {
            out.extend(self.loader.registry.ty(t).instance_fields.iter().copied());
        }
        out
    }

    fn field_slot(&self, type_id: TypeId, field: FieldId) -> Option<usize> {
        self.instance_fields(type_id).iter().position(|f| *f == field)
    }

    /// The initial value of a local, resolving value types to their zero.
    fn default_local(&mut self, assembly: AssemblyId, sig: &TypeSig) -> Value {
        if let TypeSig::ValueType(token) = sig.unwrap_modifiers() {
            if let Some(type_id) = self
                .loader
                .resolve_type_token(self.loader.assembly(assembly), *token)
            {
                return self.zero_of(type_id);
            }
        }
        self.default_value_for(sig)
    }

    pub fn default_value_for(&self, sig: &TypeSig) -> Value {
        match sig.unwrap_modifiers() {
            TypeSig::Boolean
            | TypeSig::Char
            | TypeSig::I1
            | TypeSig::U1
            | TypeSig::I2
            | TypeSig::U2
            | TypeSig::I4
            | TypeSig::U4 => Value::I32(0),
            TypeSig::I8 | TypeSig::U8 => Value::I64(0),
            TypeSig::IntPtr | TypeSig::UIntPtr => Value::NativeInt(0),
            TypeSig::R4 | TypeSig::R8 => Value::F(0.0),
            TypeSig::ValueType(_) => Value::I32(0),
            _ => Value::Null,
        }
    }

    pub fn alloc_array(&mut self, element_type: TypeId, length: usize) -> Handle {
        let ty = self.loader.registry.ty(element_type);
        let primitive = ty.primitive;
        let is_reference = !ty.kind.is_value_like();
        let array_type = self
            .loader
            .registry
            .find_sz_array(element_type)
            .unwrap_or_else(|| self.loader.core().array);
        let storage = ArrayStorage::zeroed(primitive, is_reference, length);
        self.stats.allocations += 1;
        self.heap.alloc(ClrArray {
            array_type,
            element_type,
            storage,
            dimensions: vec![length as u32],
        })
    }

    /// Allocates an array whose elements are untyped evaluation-stack values.
    ///
    /// This is the storage the generic collections are built on. `object[]`
    /// would force every `int` into a box; `Values` storage holds an `I32` slot
    /// directly, and the collector still traces it because a `Value` reports
    /// the handles it carries.
    pub fn alloc_value_array(&mut self, length: usize) -> Handle {
        let element_type = self.loader.core().object;
        let array_type = self
            .loader
            .registry
            .find_sz_array(element_type)
            .unwrap_or_else(|| self.loader.core().array);
        self.stats.allocations += 1;
        self.heap.alloc(ClrArray {
            array_type,
            element_type,
            storage: ArrayStorage::Values(vec![Value::Null; length]),
            dimensions: vec![length as u32],
        })
    }

    pub fn box_value(&mut self, type_id: TypeId, value: Value) -> Handle {
        self.stats.allocations += 1;
        self.heap.alloc(ClrBox { type_id, value })
    }

    pub fn type_of(&self, handle: Handle) -> Option<TypeId> {
        // `with_any` keeps the borrow inside the lock; see `SharedHeap`.
        self.heap.with_any(handle, |obj| {
        let any = obj.as_any();
        if let Some(o) = any.downcast_ref::<ClrObject>() {
            Some(o.type_id)
        } else if any.downcast_ref::<ClrString>().is_some() {
            Some(self.loader.core().string)
        } else if let Some(a) = any.downcast_ref::<ClrArray>() {
            Some(a.array_type)
        } else if let Some(b) = any.downcast_ref::<ClrBox>() {
            Some(b.type_id)
        } else if let Some(d) = any.downcast_ref::<ClrDelegate>() {
            Some(d.type_id)
        } else if let Some(e) = any.downcast_ref::<ClrException>() {
            Some(e.type_id)
        } else {
            Some(self.loader.core().object)
        }
        })
        .flatten()
    }

    /// The `System.Type` instance describing `type_id`, allocated once.
    ///
    /// Reflection identity matters: `typeof(int) == typeof(int)` is reference
    /// equality on .NET, and code does rely on it. Interning here gives that
    /// for free and keeps the object reachable for the process's life.
    pub fn type_object(&mut self, type_id: TypeId) -> Handle {
        if let Some(h) = self.type_objects.get(&type_id) {
            if self.heap.is_valid(*h) {
                return *h;
            }
        }
        let Some(type_type) = self.loader.registry.find_type_by_name("System.Type") else {
            return Handle::NULL;
        };
        let handle = self.alloc_object(type_type);
        self.heap.with_mut::<ClrObject, _>(handle, |o| {
            if o.fields.is_empty() {
                o.fields.push(Value::I32(type_id.0 as i32));
            } else {
                o.fields[0] = Value::I32(type_id.0 as i32);
            }
        });
        self.type_objects.insert(type_id, handle);
        handle
    }

    /// The runtime type a `System.Type` instance describes.
    pub fn type_from_object(&self, handle: Handle) -> Option<TypeId> {
        // Copied out rather than held: the borrow ends before the loader is
        // consulted, which is what an accessor behind a lock will require.
        let described = self
            .heap
            .with::<ClrObject, _>(handle, |o| (o.type_id, o.fields.first().and_then(|f| f.as_i32())))?;
        let type_type = self.loader.registry.find_type_by_name("System.Type")?;
        if described.0 != type_type {
            return None;
        }
        let id = described.1?;
        let id = TypeId(id as u32);
        if id.index() < self.loader.registry.type_count() {
            Some(id)
        } else {
            None
        }
    }

    pub fn type_name_of(&self, handle: Handle) -> String {
        self.type_of(handle)
            .map(|t| self.loader.registry.ty(t).full_name())
            .unwrap_or_else(|| "<unknown>".into())
    }

    // -- GC ------------------------------------------------------------------

    fn roots(&self) -> Vec<Handle> {
        let mut out = Vec::new();
        for frame in &self.frames {
            for v in frame.args.iter().chain(&frame.locals).chain(&frame.stack) {
                v.trace_handles(&mut out);
            }
        }
        self.loader.static_roots(&mut out);
        out.extend(self.interned.values().copied());
        out.extend(self.literal_cache.values().copied());
        out.extend(self.type_objects.values().copied());
        out
    }

    /// Collects if the policy asks for it. Only called between instructions,
    /// where no interior state is mid-update — which is also what makes this
    /// point safe to *stop* at, so it doubles as the safepoint poll.
    ///
    /// The two jobs are deliberately at the same place. A thread that checks
    /// whether it should collect is a thread that has just finished an
    /// instruction, and that is exactly when its roots are describable.
    pub fn maybe_collect(&mut self) {
        // Park here if another thread is collecting. Cheap when nobody is:
        // one relaxed load of the `stopping` flag.
        self.mutators.poll(|| self.roots());

        if self.heap.should_collect() {
            self.force_collect();
        }
    }

    pub fn force_collect(&mut self) {
        let roots = self.roots();
        let heap = self.heap.clone();
        // Every other registered thread parks before `collect` runs, and each
        // hands in its own roots on the way. With one thread this reduces to
        // collecting against `roots` and costs a flag check.
        self.mutators.stop_the_world(roots, |all| heap.collect(all));
        self.stats.collections += 1;
    }

    /// An interpreter that runs the same program on another thread.
    ///
    /// Shares the heap, the mutator registry, the native bindings and static
    /// field storage; **copies** the loader. That sounds like two runtimes and
    /// is one, because a loader is finished by the time anything runs: the copy
    /// has the same `TypeId`s and `MethodId`s as the original, so the two read
    /// identical tables and neither pays a lock to do it. See [`Loader`] on why
    /// that holds, and [`Interpreter::diverged`] on what would break it.
    ///
    /// The two are then mutator threads over one object graph: either can
    /// allocate, either can mutate an object the other made, either sees the
    /// other's writes to a static, and a collection on either stops both.
    #[cfg(feature = "std")]
    pub fn worker(&self, host: Box<dyn Host>) -> Self {
        let mut worker = Self::with_host(host);
        worker.heap = self.heap.clone();
        worker.mutators = self.mutators.clone();
        // Registration follows the registry. Assigning here drops the
        // membership `with_host` took in the throwaway registry it made.
        worker._registration = worker.mutators.register();
        worker.limits = self.limits.clone();

        // One set of monitors, or `lock` would exclude nothing.
        worker.monitors = self.monitors.clone();
        // The pool is shared so a worker can queue work and help drain it, but
        // it is *not* started by this: a pool building its own workers would
        // recurse forever.
        worker.pool = self.pool.clone();
        worker.loader = self.loader.clone();
        worker.loader.share_statics_with(&self.loader);
        // The bindings are the same program's, and a native the parent
        // registered has to be reachable from the worker or the two would run
        // different code.
        worker.natives = self.natives.clone();
        // Interned strings and `System.Type` instances are identity-bearing:
        // `typeof(T) == typeof(T)` and `ReferenceEquals` on an interned string
        // must hold across threads, so the caches come across rather than being
        // rebuilt into different objects.
        worker.interned = self.interned.clone();
        worker.literal_cache = self.literal_cache.clone();
        worker.type_objects = self.type_objects.clone();
        worker.loaded_size = self.loader.registry.method_count();
        worker
    }

    /// Runs `body` on a real OS thread and returns a token for joining it.
    ///
    /// The thread gets a worker: same heap, same statics, same bindings, its
    /// own frames. It is a *managed* thread in every sense that matters — it
    /// allocates into the same object graph, it is stopped by a collection, and
    /// what it writes to a static is what the starting thread reads.
    #[cfg(feature = "std")]
    pub fn spawn(
        &mut self,
        host: Box<dyn Host>,
        body: impl FnOnce(&mut Interpreter) -> ExecResult<()> + Send + 'static,
    ) -> u64 {
        let mut worker = self.worker(host);
        let handle = std::thread::spawn(move || {
            let result = body(&mut worker);
            // The worker's loader is dropped here, and with it its registration
            // in the mutator registry — a collection will stop waiting for a
            // thread that has gone.
            (result, worker.diverged())
        });
        self.next_thread_id = self.next_thread_id.wrapping_add(1).max(1);
        let id = self.next_thread_id;
        self.threads.insert(id, handle);
        id
    }

    /// How many other mutators are registered.
    ///
    /// Zero means nothing else is running managed code, so a task still pending
    /// will stay that way — which is the difference between waiting and
    /// hanging.
    pub fn other_threads_running(&self) -> usize {
        self.mutators.registered().saturating_sub(1)
    }

    /// Whether this interpreter started the thread with that id.
    ///
    /// Join handles belong to the interpreter that spawned them, so a thread
    /// waiting on a task another thread started has to watch instead of join.
    #[cfg(feature = "std")]
    pub fn owns_thread(&self, id: u64) -> bool {
        self.threads.contains_key(&id)
    }

    /// Waits for a thread started by [`Interpreter::spawn`].
    ///
    /// Waiting reaches no safe point, so it is done inside [`Self::blocking`]:
    /// a collection on the thread being waited for would otherwise wait for
    /// this one forever. That deadlock is not hypothetical — it is the first
    /// thing the safepoint protocol got wrong.
    #[cfg(feature = "std")]
    pub fn join(&mut self, id: u64) -> ExecResult<()> {
        let Some(handle) = self.threads.remove(&id) else {
            return Ok(());
        };
        let joined = self.blocking(|_| handle.join());
        match joined {
            Ok((result, diverged)) => {
                if diverged {
                    return Err(ExecutionError::Unsupported(
                        "a thread grew the type registry while another was running: this runtime                          gives each thread an identical copy of the loader, and one that grows is                          no longer identical. See docs/limitations.md."
                            .into(),
                    ));
                }
                result
            }
            // A panic in managed code is a runtime bug, not a program error.
            Err(_) => Err(ExecutionError::InvalidProgram(
                "a managed thread panicked".into(),
            )),
        }
    }

    /// The task pool, started on first use.
    #[cfg(feature = "std")]
    pub fn task_pool(&mut self) -> crate::pool::TaskPool {
        if let Some(pool) = &self.pool {
            return pool.clone();
        }
        let pool = crate::pool::TaskPool::start(self, crate::pool::default_host);
        self.pool = Some(pool.clone());
        pool
    }

    /// Queues managed work on the pool.
    #[cfg(feature = "std")]
    pub fn queue_work(
        &mut self,
        job: Box<dyn FnOnce(&mut Interpreter) + Send>,
    ) {
        self.task_pool().submit(job);
    }

    /// Queues managed work to run after a delay.
    #[cfg(feature = "std")]
    pub fn queue_work_after(
        &mut self,
        delay: core::time::Duration,
        job: Box<dyn FnOnce(&mut Interpreter) + Send>,
    ) {
        self.task_pool().schedule(delay, job);
    }

    /// Announces this thread blocked until the guard drops, holding no roots.
    ///
    /// For a pool worker waiting for work: it is between jobs, so its frames
    /// are empty and there is nothing for a collection to keep.
    #[cfg(feature = "std")]
    pub fn blocked_now(&self) -> rustclr_gc::safepoint::Blocked {
        self.mutators.blocked()
    }

    /// Runs one queued job here, if there is one. Used by a thread that would
    /// otherwise idle waiting for a task.
    #[cfg(feature = "std")]
    pub fn help_with_queued_work(&mut self) -> bool {
        let Some(pool) = self.pool.clone() else { return false };
        pool.run_one(self)
    }

    /// Whether anything could still complete a pending task.
    ///
    /// Pool workers are always registered, so counting mutators alone would say
    /// "yes" forever. What matters is whether work is outstanding, or a thread
    /// that is not a pool worker is running.
    #[cfg(feature = "std")]
    pub fn work_may_still_arrive(&self) -> bool {
        let pooled = self.pool.as_ref().map(|p| p.worker_count()).unwrap_or(0);
        let outstanding = self.pool.as_ref().map(|p| p.outstanding()).unwrap_or(0);
        outstanding > 0 || self.mutators.registered().saturating_sub(1 + pooled) > 0
    }

    /// Takes the monitor on `object` — the `Enter` half of `lock (object)`.    /// Takes the monitor on `object` — the `Enter` half of `lock (object)`.
    ///
    /// Blocks until it is free. The wait is announced to the collector, since
    /// a thread queued on a lock reaches no safe point of its own.
    pub fn monitor_enter(&mut self, object: Handle) {
        let monitors = self.monitors.clone();
        let mutators = self.mutators.clone();
        let roots = self.roots();
        monitors.enter(object, &mut |wait| {
            let guard = mutators.blocked_with(roots.clone());
            wait();
            drop(guard);
        });
    }

    /// Releases one level of the monitor on `object`.
    pub fn monitor_exit(&mut self, object: Handle) {
        self.monitors.exit(object);
    }

    /// Whether this thread holds the monitor on `object`.
    pub fn monitor_held(&self, object: Handle) -> bool {
        self.monitors.holds(object)
    }

    /// Runs `f` with no other thread inside an interlocked operation.
    ///
    /// `Interlocked.Increment` is a read, an add and a write, and each of those
    /// takes the heap or statics lock separately — so without this, two threads
    /// interleave between them and one increment is lost. The lock is the
    /// monitor on a handle no object can have, which reuses machinery that
    /// already announces itself to the collector.
    pub fn interlocked<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.monitor_enter(Handle::NULL);
        let out = f(self);
        self.monitor_exit(Handle::NULL);
        out
    }

    /// Whether this interpreter's loader has grown since it was copied.
    ///
    /// Two loaders that were identical stay identical only while neither adds
    /// to its registry, and a `MethodId` minted on one thread means nothing on
    /// another — it would name a different method, or none. Growth is rare and
    /// bounded: an interface stub for a native implementation, or
    /// `MakeGenericType`. This is how a thread notices it happened.
    pub fn diverged(&self) -> bool {
        self.loaded_size != 0 && self.loader.registry.method_count() != self.loaded_size
    }

    /// Bytes an instance of `type_id` occupies, for marshalling.
    pub fn size_of_public(&self, type_id: TypeId) -> usize {
        self.size_of(type_id)
    }

    /// A pointer to a fresh byte buffer of `size` bytes.
    pub fn alloc_pointer(&mut self, size: usize) -> crate::value::RawPtr {
        let buffer = self.alloc_byte_buffer(size);
        crate::value::RawPtr { buffer, offset: 0 }
    }

    /// Reads `width` bytes through a pointer.
    pub fn read_pointer(
        &mut self,
        p: crate::value::RawPtr,
        width: usize,
    ) -> ExecResult<Value> {
        self.load_pointer_sized(p, width)
    }

    /// Writes `width` bytes through a pointer.
    pub fn write_pointer(
        &mut self,
        p: crate::value::RawPtr,
        value: Value,
        width: usize,
    ) -> ExecResult<()> {
        let op = match width {
            1 => Op::StindI1,
            2 => Op::StindI2,
            8 => Op::StindI8,
            _ => Op::StindI4,
        };
        self.store_through_pointer(p, value, op)
    }

    /// `cpblk`, for the BCL's `Unsafe.CopyBlock`.
    pub fn copy_block_public(&mut self, to: Value, from: Value, count: usize) -> ExecResult<()> {
        self.copy_block(to, from, count)
    }

    /// `initblk`, for the BCL's `Unsafe.InitBlock`.
    pub fn fill_block_public(&mut self, to: Value, fill: u8, count: usize) -> ExecResult<()> {
        self.fill_block(to, fill, count)
    }

    /// The mutator registry this interpreter belongs to.
    ///
    /// A second interpreter built on the same registry and the same
    /// [`SharedHeap`] is a second mutator thread: allocation is serialised by
    /// the heap lock, and collection stops both.
    pub fn mutators(&self) -> &Mutators {
        &self.mutators
    }

    /// Marks a stretch of work that reaches no safepoint — a blocking call,
    /// or native code the runtime cannot interrupt.
    ///
    /// A collection may proceed while this is held; the thread's roots were
    /// recorded when it entered. Dropping the guard is itself a safepoint, so
    /// a thread that returns mid-collection waits rather than running on a
    /// heap being swept.
    pub fn blocking<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        // The roots go with it. A thread waiting in `Join` still holds every
        // local in its frames, and a collection that ran while it was away used
        // to sweep them.
        let guard = self.mutators.blocked_with(self.roots());
        let out = f(self);
        drop(guard);
        out
    }

    // -- compilation ---------------------------------------------------------

    pub fn compile(&mut self, method: MethodId) -> ExecResult<Arc<CompiledMethod>> {
        if let Some(c) = self.code_cache.get(&method) {
            return Ok(c.clone());
        }
        let info = self.loader.registry.method(method);
        let MethodKind::Il(body) = &info.kind else {
            return Err(ExecutionError::MissingImplementation(format!(
                "{} has no IL body",
                info.qualified_name
            )));
        };

        let instructions = decode_all(&body.il)?;
        let mut index_of_offset = HashMap::new();
        for (i, ins) in instructions.iter().enumerate() {
            index_of_offset.insert(ins.offset, i);
        }
        if let Some(last) = instructions.last() {
            // Branching to the end of the method falls out of it.
            index_of_offset.insert(last.next_offset(), instructions.len());
        }

        let compiled = Arc::new(CompiledMethod {
            method,
            instructions,
            index_of_offset,
            max_stack: body.max_stack,
            local_types: body.locals.clone(),
            exception_clauses: body.exception_clauses.clone(),
            init_locals: body.init_locals,
            arg_count: info.arg_count(),
        });
        self.code_cache.insert(method, compiled.clone());
        Ok(compiled)
    }

    // -- entry points --------------------------------------------------------

    /// Runs a method to completion and returns its result.
    pub fn invoke(&mut self, method: MethodId, args: Vec<Value>) -> ExecResult<Option<Value>> {
        let base = self.frames.len();
        // Mark the floor for this invocation. A method returning *to* the floor
        // is returning to this call, not to the managed frame underneath it —
        // without which its result would be pushed onto an unrelated
        // evaluation stack and lost to the caller here.
        let previous_floor = core::mem::replace(&mut self.frame_floor, base);
        let result = match self.enter(method, args) {
            Ok(Entered::Native(v)) => Ok(v),
            Ok(Entered::Frame) => self.run_until(base),
            Err(e) => Err(e),
        };
        self.frame_floor = previous_floor;
        result
    }

    /// Runs the entry point of an assembly and returns its exit code.
    pub fn run_entry_point(&mut self, assembly: AssemblyId) -> ExecResult<i32> {
        let entry = self.loader.assembly(assembly).entry_point.ok_or_else(|| {
            ExecutionError::MissingImplementation("assembly has no entry point".into())
        })?;

        let method = self
            .loader
            .resolve_method_token(self.loader.assembly(assembly), entry)
            .ok_or(ExecutionError::UnresolvedToken {
                token: entry,
                context: "entry point".into(),
            })?;

        let takes_args = !self.loader.registry.method(method).signature.params.is_empty();
        let args = if takes_args {
            let string_type = self.loader.core().string;
            let host_args: Vec<String> = self.host.args().to_vec();
            let array = self.alloc_array(string_type, host_args.len());
            for (i, a) in host_args.iter().enumerate() {
                let s = self.alloc_string(a);
                self.heap.with_mut::<ClrArray, _>(array, |arr| {
                    arr.storage.set(i, &Value::Obj(s));
                });
            }
            vec![Value::Obj(array)]
        } else {
            Vec::new()
        };

        let result = self.invoke(method, args)?;
        if let Some(code) = self.exit_requested {
            return Ok(code);
        }
        Ok(match result {
            Some(Value::I32(v)) => v,
            Some(Value::I64(v)) => v as i32,
            _ => 0,
        })
    }

    /// Enters a method: either pushes a frame or runs it natively.
    fn enter(&mut self, method: MethodId, args: Vec<Value>) -> ExecResult<Entered> {
        self.stats.calls += 1;

        let declaring = self.loader.registry.method(method).declaring_type;
        self.ensure_cctor(declaring)?;

        let kind_is_il = matches!(self.loader.registry.method(method).kind, MethodKind::Il(_));
        if !kind_is_il {
            let value = self.call_non_il(method, &args)?;
            return Ok(Entered::Native(value));
        }

        // Offer the call to the code generator before building a frame. A
        // compiled method needs no interpreter state at all, so this is where
        // the saving is: no frame, no locals vector, no decode loop.
        if let Some(tier) = self.native_tier.as_mut() {
            if let Some(outcome) = tier.try_execute(&self.loader, &self.heap, method, &args) {
                self.stats.native_tier_calls += 1;
                return outcome.map(Entered::Native);
            }
        }

        if self.frames.len() >= self.limits.max_frames {
            return Err(ExecutionError::exception(
                ClrExceptionKind::StackOverflow,
                format!("managed call depth exceeded {} frames", self.limits.max_frames),
            ));
        }

        let code = self.compile(method)?;
        let info = self.loader.registry.method(method);
        let assembly = info.assembly;

        // Missing arguments are a verifier error, but tolerate them as defaults
        // so a malformed call reports as a stack imbalance rather than panicking.
        let mut args = args;
        args.resize(code.arg_count.max(args.len()), Value::Null);

        // Value-type locals need their real zero, which means resolving the
        // local signature against the declaring assembly rather than guessing
        // from the signature shape alone.
        let local_types = code.local_types.clone();
        let locals: Vec<Value> = local_types
            .iter()
            .map(|t| {
                if code.init_locals {
                    self.default_local(assembly, t)
                } else {
                    Value::Null
                }
            })
            .collect();

        let id = self.next_frame_id;
        self.next_frame_id = self.next_frame_id.wrapping_add(1).max(1);

        self.frames.push(Frame {
            method,
            assembly,
            id,
            code,
            args,
            locals,
            stack: Vec::with_capacity(8),
            pc: 0,
            pending_finallies: Vec::new(),
            finally_resume: None,
            in_flight: None,
            constrained: None,
            construction: None,
            pending_newobj: None,
            pending_newobj_is_cell: false,
            is_filter: false,
        });
        self.stats.max_frame_depth = self.stats.max_frame_depth.max(self.frames.len());
        Ok(Entered::Frame)
    }

    /// Runs a method that has no IL body: native, P/Invoke or runtime-provided.
    /// Type arguments for the *next* non-IL call, taken from its call site.
    ///
    /// Consumed by `call_non_il`, so they belong to one call and cannot leak
    /// into the next one.
    pub(crate) fn stage_type_arguments(&mut self, args: Vec<TypeId>) {
        self.pending_type_args = args;
    }

    fn call_non_il(&mut self, method: MethodId, args: &[Value]) -> ExecResult<Option<Value>> {
        let info = self.loader.registry.method(method);
        let qualified = info.qualified_name.clone();

        match &info.kind {
            MethodKind::InternalCall | MethodKind::RuntimeProvided | MethodKind::Abstract => {
                self.stats.native_calls += 1;
                for key in self.loader.native_keys(method) {
                    if let Some(f) = self.natives.get(&key).copied() {
                        let previous = self.current_native.replace(method);
                        let previous_args = core::mem::take(&mut self.pending_type_args);
                        let previous_args =
                            core::mem::replace(&mut self.current_native_type_args, previous_args);
                        let result = f(self, args);
                        self.current_native = previous;
                        self.current_native_type_args = previous_args;
                        return result;
                    }
                }
                // Delegate `Invoke` and array accessors are provided by the
                // runtime itself rather than the native table.
                if let Some(v) = self.try_runtime_intrinsic(method, args)? {
                    return Ok(v);
                }
                Err(ExecutionError::MissingImplementation(qualified))
            }
            MethodKind::PInvoke { library, entry_point, .. } => {
                let key = format!("pinvoke:{library}!{entry_point}");
                if let Some(f) = self.natives.get(&key).copied() {
                    self.stats.native_calls += 1;
                    let previous = self.current_native.replace(method);
                    let result = f(self, args);
                    self.current_native = previous;
                    return result;
                }
                Err(ExecutionError::exception(
                    ClrExceptionKind::EntryPointNotFound,
                    format!("Unable to find an entry point named '{entry_point}' in DLL '{library}'."),
                ))
            }
            MethodKind::Il(_) => unreachable!("checked by the caller"),
        }
    }

    /// Handles members the runtime implements structurally.
    fn try_runtime_intrinsic(
        &mut self,
        method: MethodId,
        args: &[Value],
    ) -> ExecResult<Option<Option<Value>>> {
        let info = self.loader.registry.method(method);
        let name = info.name.clone();
        let declaring = info.declaring_type;
        let is_delegate = matches!(self.loader.registry.ty(declaring).kind, TypeKind::Delegate);

        if is_delegate && name == "Invoke" {
            let Some(Value::Obj(h)) = args.first() else {
                return Err(ExecutionError::null_reference());
            };
            let targets = self
                .heap
                .with::<ClrDelegate, _>(*h, |d| d.targets.clone())
                .ok_or_else(ExecutionError::null_reference)?;

            let mut result = None;
            for target in targets {
                let mut call_args = Vec::with_capacity(args.len());
                if !target.receiver.is_null() {
                    call_args.push(Value::Obj(target.receiver));
                }
                call_args.extend_from_slice(&args[1..]);
                // A multicast delegate returns the last invocation's result.
                result = self.invoke(target.method, call_args)?;
            }
            return Ok(Some(result));
        }

        Ok(None)
    }

    /// Runs the static constructor of a type if it has not run yet.
    pub fn ensure_cctor(&mut self, type_id: TypeId) -> ExecResult<()> {
        if !type_id.is_valid() {
            return Ok(());
        }
        let ty = self.loader.registry.ty(type_id);
        match ty.cctor_state {
            CctorState::Done | CctorState::Running => return Ok(()),
            CctorState::Failed => {
                return Err(ExecutionError::exception(
                    ClrExceptionKind::TypeLoad,
                    format!("The type initializer for '{}' threw an exception.", ty.full_name()),
                ))
            }
            CctorState::NotRun => {}
        }

        let cctor = ty.cctor;
        self.loader.registry.ty_mut(type_id).cctor_state = CctorState::Running;

        if let Some(cctor) = cctor {
            match self.invoke(cctor, Vec::new()) {
                Ok(_) => {}
                Err(e) => {
                    self.loader.registry.ty_mut(type_id).cctor_state = CctorState::Failed;
                    return Err(e);
                }
            }
        }
        self.loader.registry.ty_mut(type_id).cctor_state = CctorState::Done;
        Ok(())
    }

    // -- the loop ------------------------------------------------------------

    fn run_until(&mut self, base_depth: usize) -> ExecResult<Option<Value>> {
        let mut result = None;

        while self.frames.len() > base_depth {
            if let Some(budget) = self.limits.max_instructions {
                if self.stats.instructions_executed >= budget {
                    return Err(ExecutionError::InstructionBudgetExceeded(budget));
                }
            }

            match self.step() {
                Ok(StepOutcome::Continue) => {}
                Ok(StepOutcome::Returned(v)) => {
                    if self.frames.len() == base_depth {
                        result = v;
                    }
                }
                Err(e) if e.is_managed_exception() => {
                    self.dispatch_exception(e, base_depth)?;
                }
                Err(e) => return Err(e),
            }

            if self.exit_requested.is_some() {
                self.frames.truncate(base_depth);
                break;
            }
        }

        Ok(result)
    }

    fn step(&mut self) -> ExecResult<StepOutcome> {
        let top = self.frames.len() - 1;
        let (code, pc) = {
            let f = &self.frames[top];
            (f.code.clone(), f.pc)
        };

        let Some(instruction) = code.instructions.get(pc).cloned() else {
            // Falling off the end without `ret` is invalid IL; return void
            // rather than spinning.
            return self.do_return(None);
        };

        self.stats.instructions_executed += 1;
        self.frames[top].pc = pc + 1;
        self.execute(&instruction)
    }

    // -- frame accessors -----------------------------------------------------

    #[inline]
    fn frame(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("a frame is active")
    }

    #[inline]
    fn frame_ref(&self) -> &Frame {
        self.frames.last().expect("a frame is active")
    }

    #[inline]
    fn push(&mut self, v: Value) {
        self.frame().stack.push(v);
    }

    fn pop(&mut self) -> ExecResult<Value> {
        let method = self.frame_ref().method;
        self.frame().stack.pop().ok_or_else(|| {
            let name = self.loader.registry.method(method).qualified_name.clone();
            ExecutionError::StackImbalance {
                at: name,
                detail: "pop from an empty evaluation stack".into(),
            }
        })
    }

    fn pop2(&mut self) -> ExecResult<(Value, Value)> {
        let b = self.pop()?;
        let a = self.pop()?;
        Ok((a, b))
    }

    fn branch_to(&mut self, offset: u32) -> ExecResult<()> {
        let index = self.frame_ref().code.index_for(offset)?;
        self.frame().pc = index;
        Ok(())
    }

    // -- returning -----------------------------------------------------------

    fn do_return(&mut self, value: Option<Value>) -> ExecResult<StepOutcome> {
        let finished = self.frames.pop();
        let below_floor = self.frames.len() <= self.frame_floor;
        // A `.ctor` returns void, but `newobj` must leave the new instance on
        // the caller's stack.
        let value = value.or_else(|| match finished {
            Some(frame) => match (frame.pending_newobj, frame.pending_newobj_is_cell) {
                // A value type was constructed into a temporary cell; hand the
                // caller the constructed value, not the cell.
                (Some(cell), true) => self
                    .heap
                    .with::<ClrObject, _>(cell, |o| o.fields.first().cloned())
                    .flatten(),
                (Some(handle), false) => Some(Value::Obj(handle)),
                _ => None,
            },
            None => None,
        });
        // Returning to or below the floor hands the value back to whoever
        // started this invocation — the top-level runner, or a native method
        // that called into managed code.
        if below_floor {
            return Ok(StepOutcome::Returned(value));
        }
        match (self.frames.last_mut(), value) {
            (Some(caller), Some(v)) => {
                caller.stack.push(v);
                Ok(StepOutcome::Continue)
            }
            (Some(_), None) => Ok(StepOutcome::Continue),
            (None, v) => Ok(StepOutcome::Returned(v)),
        }
    }
}

// Instruction dispatch and exception handling live in submodules purely to
// keep this file navigable; they extend `Interpreter` in place.
mod arith;
mod eh;
mod exec;
