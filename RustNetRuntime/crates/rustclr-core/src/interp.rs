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
use rustclr_gc::{GcObject, Handle, SharedHeap};
use rustclr_metadata::{ExceptionClause, HandlerKind, Token, TypeSig};

#[allow(unused_imports)]
use crate::prelude::*;


/// A method decoded and prepared for execution.
#[derive(Debug)]
pub struct CompiledMethod {
    pub method: MethodId,
    pub instructions: Vec<Instruction>,
    /// IL offset to instruction index; branch targets are IL offsets.
    ///
    /// A flat table indexed by offset rather than a map: every taken branch
    /// consults it, and a hash lookup there cost more than the branch itself.
    /// Offsets that start no instruction hold `u32::MAX`.
    pub index_of_offset: Vec<u32>,
    pub max_stack: u16,
    pub local_types: Vec<TypeSig>,
    pub exception_clauses: Vec<ExceptionClause>,
    pub init_locals: bool,
    pub arg_count: usize,
    /// Each local's starting value. It depends only on the local's signature
    /// and the method's assembly, so it is resolved once here rather than on
    /// every call.
    pub local_defaults: Vec<Value>,
    /// Struct locals of a generic type over the method's own type parameters
    /// — an async method's state machine, `Pair<T>` — which only the frame can
    /// zero properly, since their construction depends on the call.
    pub generic_struct_locals: Vec<(usize, TypeSig)>,
}

impl CompiledMethod {
    fn index_for(&self, offset: u32) -> ExecResult<usize> {
        match self.index_of_offset.get(offset as usize) {
            Some(&i) if i != u32::MAX => Ok(i as usize),
            _ => Err(ExecutionError::InvalidProgram(format!("branch to invalid IL offset {offset:#x}"))),
        }
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
    /// `mutators` is the safepoint registry, and `interpreter_roots` produces
    /// the roots of the frames *below* this call. Both are needed because a
    /// compiled method may now reach a safe point in the middle of a loop: the
    /// thread that parks there is this one, and the collection it parks for
    /// needs its whole root set — the compiled frame's handles, which the
    /// backend knows how to find, *and* the interpreter frames underneath,
    /// which it does not.
    ///
    /// The closure is only called if a collection is actually requested, so
    /// walking those frames costs nothing on the ordinary path.
    fn try_execute(
        &mut self,
        loader: &Loader,
        heap: &SharedHeap,
        method: MethodId,
        args: &[Value],
        mutators: &Mutators,
        interpreter_roots: &mut dyn FnMut() -> Vec<Handle>,
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
    /// The native bound to each method, by method index, once looked up:
    /// `Some(None)` records a method with no native.
    native_cache: Vec<Option<Option<NativeFn>>>,
    /// Spare frame vectors; see `frame_vec`.
    vec_pool: Vec<Vec<Value>>,
    /// One-slot cells a struct constructor ran against, kept for the next
    /// `newobj` of a value type. Rooted, so the collector leaves them alone.
    pub(crate) struct_cells: Vec<Handle>,
    /// Optional native code generator. `None` means everything is interpreted.
    pub native_tier: Option<Box<dyn NativeTier>>,
    /// Frame depth below which a `ret` is returning out of the current
    /// invocation rather than into a managed caller. Zero at the top level;
    /// raised while a native method calls back into managed code.
    frame_floor: usize,
    /// Decoded methods by method index.
    code_cache: Vec<Option<Arc<CompiledMethod>>>,
    /// The starting field values for each type, remembered per type.
    ///
    /// Every instance of a type begins identically, and working that out means
    /// walking the base chain, building two vectors and reading a signature per
    /// field. Doing it per *allocation* made `alloc` the slowest row in the
    /// benchmark table by a distance; doing it per type makes an allocation one
    /// vector clone.
    field_defaults: crate::rowmap::RowMap<Vec<Value>>,
    /// The flattened instance-field list for each type, remembered per type.
    ///
    /// The registry stores each type's *own* fields; this is that list
    /// concatenated base-first, which is what slot numbers are relative to.
    instance_field_cache: HashMap<TypeId, Arc<Vec<FieldId>>>,
    /// State a native library keeps for itself, by name: compiled regular
    /// expressions, for one. Native bindings are plain function pointers with
    /// nowhere else to put a cache that outlives a call.
    pub native_state: alloc::collections::BTreeMap<&'static str, Box<dyn core::any::Any + Send>>,
    /// The slot each instance field occupies, remembered per field.
    ///
    /// A field's position does not depend on the receiver's runtime type. The
    /// flattened layout is base-first, so a field declared in `T` sits at the
    /// same index in `T` and in every subclass of `T` — which is what makes
    /// this cacheable by field alone, and what lets a field access skip
    /// reading the object's type at all. That read was a second trip through
    /// the heap's lock for every `ldfld` and `stfld` in the program.
    field_slots: crate::rowmap::RowMap<usize>,
    /// How a thread this program starts gets its console.
    ///
    /// `None` means real stdio, which is right for `rustnet run` and wrong for
    /// `rustnet debug --dap`: there stdout carries the protocol, and a
    /// `Console.WriteLine` from a worker thread landing in the middle of a
    /// framed message corrupts it. The embedder that redirected the main
    /// thread's console has to be able to redirect a spawned thread's too, so
    /// the host is *built by it* rather than assumed here.
    #[cfg(feature = "std")]
    pub host_factory: Option<alloc::sync::Arc<dyn Fn() -> Box<dyn Host> + Send + Sync>>,
    /// The board underneath, if the embedder installed one.
    ///
    /// `None` is a desktop with no hardware: `RustNet.Hal` calls then fail
    /// with a message saying no board is installed, rather than pretending a
    /// pin was written. The firmware for a real chip installs its own; a test
    /// or the CLI installs the simulator.
    ///
    /// It sits here for the same reason the host does: a native binding is
    /// handed `&mut Interpreter` and nothing else.
    pub board: Option<Box<dyn rustclr_hal::Board>>,
    /// The drawing surface, if a program opened one.
    ///
    /// One per interpreter and not per surface: a 240x240 RGB565 buffer is
    /// 115 KB, which on most of the boards here is the whole heap budget. A
    /// second one would not be a feature, it would be an out-of-memory.
    pub canvas: Option<rustclr_gfx::Framebuffer>,
    /// The MQTT connection, if a program opened one.
    ///
    /// One per interpreter: a device has one broker and a few kilobytes, so a
    /// client pool would be a feature nothing here asked for.
    #[cfg(feature = "std")]
    pub mqtt: Option<rustclr_net::mqtt::MqttClient>,
    /// The attached profiler, if any.
    ///
    /// `None` costs one `is_some` per instruction — the same shape as the
    /// debugger hook, and for the same reason: a program run without one must
    /// not pay for the hook existing.
    pub profiler: Option<crate::profile::Profiler>,
    /// The attached debugger, if any.
    ///
    /// `None` costs one branch per instruction — the location is not built and
    /// the breakpoint set is not consulted — so a program run without one pays
    /// essentially nothing for the hook existing.
    pub debugger: Option<crate::debug::BoxedDebugger>,

    /// A device service polled while a program runs.
    ///
    /// A board that answers a host tool has to do it *during* the program, not
    /// between programs: the questions worth asking — what is on the screen,
    /// what has it logged, stop it — are only interesting about something that
    /// is running. There is no second thread on these chips to ask from, so
    /// the service is polled from the instruction loop like the profiler.
    ///
    /// See [`DevicePoll`] for why it is handed the whole interpreter.
    pub device: Option<alloc::boxed::Box<dyn DevicePoll>>,

    /// Set from outside to ask the running program to stop.
    ///
    /// Checked once per instruction at the safe point in [`Self::run_until`].
    /// `None` — the default — costs one null check.
    ///
    /// # Why a `&'static` rather than an `Arc`
    ///
    /// `alloc::sync::Arc` does not exist on `thumbv6m` or `riscv32imc`: both
    /// lack compare-and-swap, and a reference count needs it. `AtomicBool`
    /// survives on those targets because plain `load` and `store` do not, and
    /// those are the only two operations this ever needs — it is set once and
    /// read many times, never exchanged.
    ///
    /// A `&'static` is also the shape a firmware wants: the flag is a `static`
    /// in the firmware image, not something allocated per run.
    pub cancel: Option<&'static core::sync::atomic::AtomicBool>,
    /// Where the program should stop.
    pub breakpoints: crate::debug::Breakpoints,
    /// What the interpreter is stepping towards, between stops.
    stepping: Option<crate::debug::Stepping>,
    /// Whether the debugger has been told the program started.
    announced_entry: bool,
    /// Exception objects currently being dispatched.
    ///
    /// A stack rather than one handle: a filter can throw, and its dispatch
    /// nests inside the one already running. Each entry is a root until its
    /// dispatch finishes.
    dispatching: Vec<Handle>,
    /// Values a running native holds only in Rust: its arguments, and what the
    /// managed code it calls back into returns. A native's arguments are popped
    /// off the caller's stack before it runs, so without this a collection
    /// triggered inside a callback (a LINQ predicate that allocates) freed the
    /// very delegate being called, or results a `Select` had gathered so far.
    native_roots: Vec<Value>,
    native_depth: usize,
    /// Whether each constructor provably does nothing, remembered per method.
    ///
    /// [`Loader::constructor_does_nothing`] decodes IL to answer, which is far
    /// more expensive than the two frames it saves — so asking it once per
    /// `newobj` would be slower than not asking at all. Memoised here rather
    /// than on the loader because a loader is finished before the first
    /// instruction runs and every thread holds its own copy of it; this is
    /// per-interpreter state and belongs with the other per-interpreter caches.
    trivial_ctors: crate::rowmap::RowMap<bool>,
    /// Overrides of virtual generic methods, instantiated per call-site
    /// type arguments. See `resolve_virtual_target`.
    virtual_instantiations: HashMap<(MethodId, Vec<TypeId>), MethodId>,
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
    /// Whether this interpreter *is* a pool worker, rather than one that uses
    /// the pool. Only a worker's blocking should make the pool grow.
    #[cfg(feature = "std")]
    is_pool_worker: bool,
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
            native_cache: Vec::new(),
            vec_pool: Vec::new(),
            struct_cells: Vec::new(),
            native_tier: None,
            frame_floor: 0,
            code_cache: Vec::new(),
            trivial_ctors: crate::rowmap::RowMap::new(),
            virtual_instantiations: HashMap::new(),
            field_defaults: crate::rowmap::RowMap::new(),
            field_slots: crate::rowmap::RowMap::new(),
            dispatching: Vec::new(),
            native_roots: Vec::new(),
            native_depth: 0,
            board: None,
            canvas: None,
            #[cfg(feature = "std")]
            mqtt: None,
            profiler: None,
            #[cfg(feature = "std")]
            host_factory: None,
            debugger: None,
            cancel: None,
            device: None,
            breakpoints: crate::debug::Breakpoints::new(),
            stepping: None,
            announced_entry: false,
            instance_field_cache: HashMap::new(),
            native_state: alloc::collections::BTreeMap::new(),
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
            is_pool_worker: false,
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
        // A binding added later can satisfy a method an earlier lookup missed.
        self.native_cache.clear();
    }

    pub fn native_count(&self) -> usize {
        self.natives.len()
    }

    pub fn has_native(&self, key: &str) -> bool {
        self.natives.contains_key(key)
    }

    /// The native registered under a binding key, to call it directly.
    pub fn native(&self, key: &str) -> Option<NativeFn> {
        self.natives.get(key).copied()
    }

    /// Every binding key currently registered.
    ///
    /// For comparing one binding set against another — which is how the
    /// full and minimal tiers are kept in a subset relationship.
    pub fn native_binding_keys(&self) -> Vec<String> {
        self.natives.keys().cloned().collect()
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
        self.alloc_clr_string(ClrString::from_str(s))
    }

    pub fn alloc_clr_string(&mut self, s: ClrString) -> Handle {
        self.stats.allocations += 1;
        self.attribute(self.loader.core().string, s.size_hint());
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
        // Every instance of a type starts the same way, so the starting values
        // are worked out once and copied thereafter.
        let fields = self.field_defaults(type_id).to_vec();
        let obj = ClrObject { type_id, fields, monitor: None };
        self.stats.allocations += 1;
        self.attribute(type_id, obj.size_hint());
        self.heap.alloc(obj)
    }

    /// Allocates an object that has already been filled in.
    ///
    /// The counted way to put a prepared `ClrObject` on the heap. Reaching for
    /// `heap.alloc` directly skips both the allocation counter and the
    /// profiler's attribution, which is how one `await` site came to be
    /// invisible to `rustnet profile` — the totals stopped adding up, and a
    /// test now says so.
    pub fn alloc_clr_object(&mut self, obj: ClrObject) -> Handle {
        self.stats.allocations += 1;
        self.attribute(obj.type_id, obj.size_hint());
        self.heap.alloc(obj)
    }

    /// The value each instance field of `type_id` starts at.
    fn field_defaults(&mut self, type_id: TypeId) -> &[Value] {
        if !self.field_defaults.contains_key(&(type_id.index() as u32)) {
            let fields = self.instance_fields(type_id);
            let mut defaults = Vec::with_capacity(fields.len());
            for field in fields.iter() {
                // A struct-typed field starts as a zeroed struct, not a
                // placeholder: it is read through before anything assigns it.
                defaults.push(self.loader.zero_of_field(*field, 0));
            }
            self.field_defaults.insert(type_id.index() as u32, defaults);
        }
        &self.field_defaults[&(type_id.index() as u32)]
    }

    /// Instance fields of a type, base-first so slots are stable in subclasses.
    ///
    /// The registry holds each type's own fields; the flattening across the
    /// base chain is this. Kept as a `Vec` return for the callers that want to
    /// own one — [`Self::instance_field_list`] is the allocation-free form the
    /// hot paths use.
    pub fn instance_fields(&self, type_id: TypeId) -> Vec<FieldId> {
        let mut chain: Vec<TypeId> = self.loader.registry.base_chain(type_id).collect();
        chain.reverse();
        let mut out = Vec::new();
        for t in chain {
            out.extend(self.loader.registry.ty(t).instance_fields.iter().copied());
        }
        out
    }

    /// [`Self::instance_fields`], computed once per type.
    ///
    /// The answer cannot change: a type's fields are fixed when it loads.
    fn instance_field_list(&mut self, type_id: TypeId) -> Arc<Vec<FieldId>> {
        if let Some(cached) = self.instance_field_cache.get(&type_id) {
            return cached.clone();
        }
        let list = Arc::new(self.instance_fields(type_id));
        self.instance_field_cache.insert(type_id, list.clone());
        list
    }

    /// The slot `field` occupies in any instance that has it.
    ///
    /// `type_id` is the receiver's type when the caller knows it and is only
    /// consulted on a miss — the answer is a property of the field.
    fn field_slot(&mut self, type_id: TypeId, field: FieldId) -> Option<usize> {
        if let Some(&slot) = self.field_slots.get(&(field.index() as u32)) {
            return Some(slot);
        }
        // Resolve against the field's own declaring type rather than the
        // receiver's: a subclass has the same layout for the part it inherits,
        // and the declaring type is the one guaranteed to contain the field.
        let declaring = self.loader.registry.field(field).declaring_type;
        let mut slot = self.slot_within(declaring, field);
        // The receiver's own type is the fallback, for a field whose declaring
        // type does not list it — an explicit-layout or synthesised shape. It
        // is consulted only when it is a real type: callers that have no
        // receiver type pass `TypeId::INVALID`, and indexing the registry with
        // that is a panic rather than a miss. An aggressive collector found
        // that the hard way, on a path that had simply never been reached.
        if slot.is_none() && type_id != TypeId::INVALID && type_id != declaring {
            slot = self.slot_within(type_id, field);
        }
        let slot = slot?;
        self.field_slots.insert(field.index() as u32, slot);
        Some(slot)
    }

    /// Where `field` sits in `owner`'s flattened layout, if it is there at all.
    fn slot_within(&mut self, owner: TypeId, field: FieldId) -> Option<usize> {
        if owner == TypeId::INVALID || owner.index() >= self.loader.registry.type_count() {
            return None;
        }
        self.instance_field_list(owner).iter().position(|f| *f == field)
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
        let array = ClrArray {
            array_type,
            element_type,
            storage,
            dimensions: vec![length as u32],
        };
        self.attribute_array(element_type, array.size_hint());
        self.heap.alloc(array)
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
        let array = ClrArray {
            array_type,
            element_type,
            storage: ArrayStorage::Values(vec![Value::Null; length]),
            dimensions: vec![length as u32],
        };
        self.attribute_array(element_type, array.size_hint());
        self.heap.alloc(array)
    }

    pub fn box_value(&mut self, type_id: TypeId, value: Value) -> Handle {
        self.stats.allocations += 1;
        let boxed = ClrBox { type_id, value };
        self.attribute(type_id, boxed.size_hint());
        self.heap.alloc(boxed)
    }

    /// Charges an array to its element type, when something is profiling.
    #[inline]
    fn attribute_array(&mut self, element: TypeId, bytes: usize) {
        if let Some(profiler) = self.profiler.as_mut() {
            profiler.allocated_array(element, bytes);
        }
    }

    /// Charges an allocation to a type, when something is profiling.
    ///
    /// Takes the size the *collector* will account for rather than a size of
    /// its own, so "what allocated the bytes" and "how full the heap is" can
    /// never disagree.
    #[inline]
    fn attribute(&mut self, type_id: TypeId, bytes: usize) {
        if let Some(profiler) = self.profiler.as_mut() {
            profiler.allocated(type_id, bytes);
        }
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
            // Two handles a frame holds that are in none of those three.
            //
            // `pending_newobj` is the instance a constructor is running
            // against: allocated, handed to the constructor as `this`, and not
            // yet pushed anywhere. `in_flight` is the exception being
            // dispatched, which lives in a Rust field while filters and
            // finally blocks run — and those run *managed code*, which
            // allocates.
            //
            // Both were invisible to a collection. Neither could be reached
            // before, because a collection only happens where the interpreter
            // asks for one and nothing asked during those windows; an
            // aggressive collector reaches both immediately, and the symptom is
            // a swept exception object surfacing as a type index of -1.
            if let Some(pending) = frame.pending_newobj {
                out.push(pending);
            }
            if let Some(error) = &frame.in_flight {
                if let ExecutionError::Exception { object, .. } = error.as_ref() {
                    out.push(*object);
                }
            }
        }
        out.extend(self.dispatching.iter().copied());
        for v in &self.native_roots {
            v.trace_handles(&mut out);
        }
        out.extend(self.struct_cells.iter().copied());
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
        let started = self.profiler.as_ref().map(|_| self.host.monotonic_nanos());
        let roots = self.roots();
        let heap = self.heap.clone();
        // Every other registered thread parks before `collect` runs, and each
        // hands in its own roots on the way. With one thread this reduces to
        // collecting against `roots` and costs a flag check.
        self.mutators.stop_the_world(roots, |all| heap.collect(all));
        self.stats.collections += 1;
        // The whole stop-the-world, not the marking alone: what a program
        // notices is how long it was not running.
        if let Some(started) = started {
            let elapsed = self.host.monotonic_nanos().saturating_sub(started);
            if let Some(profiler) = self.profiler.as_mut() {
                profiler.collected(elapsed);
            }
        }
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
        // A thread this worker starts in turn gets the same console.
        worker.host_factory = self.host_factory.clone();
        worker
    }

    /// A console for a thread this program is about to start.
    ///
    /// Real stdio unless the embedder said otherwise. Every place that starts a
    /// managed thread goes through this, so redirecting the console redirects
    /// *all* of it rather than only the thread that happened to be first.
    #[cfg(feature = "std")]
    pub fn new_host(&self) -> Box<dyn Host> {
        match &self.host_factory {
            Some(build) => build(),
            None => Box::new(crate::host::SystemHost::new()),
        }
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
        // The pool's workers get whatever console this interpreter hands out,
        // so a redirected console reaches `Task.Run` as well as `Main`.
        let factory = self.host_factory.clone();
        let pool = crate::pool::TaskPool::start(self, move || match &factory {
            Some(build) => build(),
            None => crate::pool::default_host(),
        });
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

    /// `Monitor.Wait` — releases the monitor, waits for a pulse, retakes it.
    ///
    /// `false` when this thread does not hold the monitor.
    pub fn monitor_wait(
        &mut self,
        object: Handle,
        timeout: Option<core::time::Duration>,
    ) -> crate::monitor::Wait {
        let monitors = self.monitors.clone();
        let mutators = self.mutators.clone();
        let roots = self.roots();
        monitors.wait(object, timeout, &mut |wait| {
            let guard = mutators.blocked_with(roots.clone());
            wait();
            drop(guard);
        })
    }

    /// `Monitor.Pulse` — lets waiters on `object` re-check.
    pub fn monitor_pulse(&mut self, object: Handle) {
        self.monitors.pulse(object);
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
        // A pool worker that blocks is a worker the pool has lost for the
        // duration, so it says so — and the pool starts another if that leaves
        // queued work with nobody to run it.
        #[cfg(feature = "std")]
        let pool = self.pool.clone().filter(|_| self.is_pool_worker);
        #[cfg(feature = "std")]
        let _away = pool.as_ref().map(|p| p.worker_blocking(self));

        let out = f(self);
        drop(guard);
        out
    }

    /// Marks this interpreter as one of the pool's own workers.
    #[cfg(feature = "std")]
    pub fn mark_as_pool_worker(&mut self, pool: crate::pool::TaskPool) {
        self.pool = Some(pool);
        self.is_pool_worker = true;
    }

    // -- compilation ---------------------------------------------------------

    pub fn compile(&mut self, method: MethodId) -> ExecResult<Arc<CompiledMethod>> {
        if let Some(Some(c)) = self.code_cache.get(method.index()) {
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
        let end = instructions.last().map(|l| l.next_offset() as usize).unwrap_or(0);
        let mut index_of_offset = vec![u32::MAX; end + 1];
        for (i, ins) in instructions.iter().enumerate() {
            index_of_offset[ins.offset as usize] = i as u32;
        }
        // Branching to the end of the method falls out of it.
        index_of_offset[end] = instructions.len() as u32;

        let assembly = info.assembly;
        let init_locals = body.init_locals;
        let local_sigs = body.locals.clone();
        let local_defaults: Vec<Value> = local_sigs
            .iter()
            .map(|t| if init_locals { self.default_local(assembly, t) } else { Value::Null })
            .collect();
        let info = self.loader.registry.method(method);
        let MethodKind::Il(body) = &info.kind else { unreachable!("checked above") };
        let generic_struct_locals: Vec<(usize, TypeSig)> = if init_locals {
            local_sigs
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    matches!(t.unwrap_modifiers(), TypeSig::GenericInst { is_value_type: true, .. })
                        && crate::loader::mentions_generic_parameter(t)
                })
                .map(|(i, t)| (i, t.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let compiled = Arc::new(CompiledMethod {
            local_defaults,
            generic_struct_locals,
            method,
            instructions,
            index_of_offset,
            max_stack: body.max_stack,
            local_types: body.locals.clone(),
            exception_clauses: body.exception_clauses.clone(),
            init_locals: body.init_locals,
            arg_count: info.arg_count(),
        });
        if method.index() >= self.code_cache.len() {
            self.code_cache.resize(method.index() + 1, None);
        }
        self.code_cache[method.index()] = Some(compiled.clone());
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
        self.root_callback_result(&result);
        result
    }

    /// A value managed code hands back to a running native lives only in that
    /// native's Rust locals until it returns, so it is kept as a root until then.
    fn root_callback_result(&mut self, result: &ExecResult<Option<Value>>) {
        if self.native_depth > 0 {
            if let Ok(Some(v)) = result {
                // Structs carry handles too; tracing ignores what holds none.
                self.native_roots.push(v.clone());
            }
        }
    }

    /// [`Self::invoke`], with the frame told which generic construction it
    /// runs for.
    fn invoke_in_construction(
        &mut self,
        method: MethodId,
        args: Vec<Value>,
        construction: Option<TypeId>,
    ) -> ExecResult<Option<Value>> {
        let base = self.frames.len();
        let previous_floor = core::mem::replace(&mut self.frame_floor, base);
        let result = match self.enter(method, args) {
            Ok(Entered::Native(v)) => Ok(v),
            Ok(Entered::Frame) => {
                if construction.is_some() {
                    self.frame().construction = construction;
                }
                self.run_until(base)
            }
            Err(e) => Err(e),
        };
        self.frame_floor = previous_floor;
        self.root_callback_result(&result);
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
            let value = self.call_non_il(method, &args);
            self.recycle(args);
            return Ok(Entered::Native(value?));
        }

        // Offer the call to the code generator before building a frame. A
        // compiled method needs no interpreter state at all, so this is where
        // the saving is: no frame, no locals vector, no decode loop.
        if self.native_tier.is_some() {
            // The tier and the roots it may ask for are borrowed from the same
            // interpreter, so the pieces it needs are moved out for the call
            // and put back after. Splitting the borrow any other way would
            // mean the roots closure could not see the frames.
            let mut tier = self.native_tier.take().expect("checked above");
            let mutators = self.mutators.clone();
            let frames = &self.frames;
            let loader = &self.loader;
            let interned = &self.interned;
            let literals = &self.literal_cache;
            let cells = &self.struct_cells;
            let type_objects = &self.type_objects;
            let mut roots = || {
                let mut out = Vec::new();
                for frame in frames {
                    for v in frame.args.iter().chain(&frame.locals).chain(&frame.stack) {
                        v.trace_handles(&mut out);
                    }
                }
                loader.static_roots(&mut out);
                out.extend(interned.values().copied());
                out.extend(literals.values().copied());
                out.extend(cells.iter().copied());
                out.extend(type_objects.values().copied());
                out
            };
            let outcome =
                tier.try_execute(loader, &self.heap, method, &args, &mutators, &mut roots);
            self.native_tier = Some(tier);
            if let Some(outcome) = outcome {
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

        // Value-type locals need their real zero, resolved once per method
        // when it was compiled.
        let mut locals = self.frame_vec();
        locals.extend_from_slice(&code.local_defaults);
        let stack = self.frame_vec();

        let id = self.next_frame_id;
        self.next_frame_id = self.next_frame_id.wrapping_add(1).max(1);
        let generic_locals = if code.generic_struct_locals.is_empty() { None } else { Some(code.generic_struct_locals.clone()) };

        self.frames.push(Frame {
            method,
            assembly,
            id,
            code,
            args,
            locals,
            stack,
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
        // Zero generic struct locals as the construction this call names, so
        // a state machine's `MoveNext` can answer `typeof(T)`.
        if let Some(generic_locals) = generic_locals {
            for (index, sig) in generic_locals {
                let Some(t) = self.frame_type_of_sig(&sig) else { continue };
                if self.loader.registry.ty(t).generic_definition.is_none() {
                    continue;
                }
                let zero = self.zero_of(t);
                if let Some(slot) = self.frames.last_mut().and_then(|f| f.locals.get_mut(index)) {
                    *slot = zero;
                }
            }
        }
        if let Some(profiler) = self.profiler.as_mut() {
            profiler.entered(method);
        }
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

        match &info.kind {
            MethodKind::InternalCall | MethodKind::RuntimeProvided | MethodKind::Abstract => {
                self.stats.native_calls += 1;
                // The binding for a method never changes once found, so it is
                // looked up by name once and then by index: building both key
                // strings and searching a string-keyed table on every call
                // cost more than most of the natives themselves.
                let slot = method.index();
                let cached = match self.native_cache.get(slot).copied().flatten() {
                    Some(found) => found,
                    None => {
                        let found = self
                            .loader
                            .native_keys(method)
                            .into_iter()
                            .find_map(|key| self.natives.get(&key).copied());
                        if slot >= self.native_cache.len() {
                            self.native_cache.resize(slot + 1, None);
                        }
                        self.native_cache[slot] = Some(found);
                        found
                    }
                };
                {
                    if let Some(f) = cached {
                        let previous = self.current_native.replace(method);
                        let previous_args = core::mem::take(&mut self.pending_type_args);
                        let previous_args =
                            core::mem::replace(&mut self.current_native_type_args, previous_args);
                        let mark = self.native_roots.len();
                        self.native_roots.extend(args.iter().cloned());
                        self.native_depth += 1;
                        let result = f(self, args);
                        self.native_depth -= 1;
                        self.native_roots.truncate(mark);
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
                Err(ExecutionError::MissingImplementation(self.loader.registry.method(method).qualified_name.clone()))
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
        // An open generic definition is never initialised by itself: each
        // construction has its own statics and runs the constructor for
        // them, which `do_call` arranges from the call site.
        if ty.generic_param_count > 0 && ty.generic_args.is_empty() {
            return Ok(());
        }
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
            // A generic type's static constructor runs once per construction
            // and has no receiver, so the frame is told which construction it
            // is initialising -- otherwise `static int N = typeof(T).Name.Length`
            // has nothing to read `T` from.
            let construction = (!self.loader.registry.ty(type_id).generic_args.is_empty()).then_some(type_id);
            match self.invoke_in_construction(cctor, Vec::new(), construction) {
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

    /// Takes a sample if a sampling period has passed.
    ///
    /// Cold and never inlined, for the reason the debug hook is: without it,
    /// the branch and its registers get hoisted into the loop that runs every
    /// instruction and cost something even when it does nothing.
    #[cold]
    #[inline(never)]
    fn profile_poll(&mut self) {
        let Some(profiler) = self.profiler.as_mut() else { return };
        if !profiler.tick() {
            return;
        }
        let now = self.host.monotonic_nanos();
        // The stack is copied out because `sample` needs `&mut` on the
        // profiler while it reads the frames. It happens once per interval,
        // not once per instruction.
        let stack: Vec<crate::MethodId> = self.frames.iter().map(|f| f.method).collect();
        if let Some(profiler) = self.profiler.as_mut() {
            profiler.sample(now, &stack);
        }
    }

    /// Decides whether to stop here, and stops if so.
    ///
    /// `cold` and never inlined: this is called from the instruction loop and
    /// answers "no" on every instruction of every program that is not being
    /// debugged. Letting it be inlined puts its registers and its code in the
    /// hot path for the benefit of a branch that is essentially never taken.
    ///
    /// Only called with a debugger attached. Everything expensive — describing
    /// frames, naming methods — happens after the decision, never before it.
    #[cold]
    #[inline(never)]
    fn debug_poll(&mut self, il_offset: u32) {
        let depth = self.frames.len();
        let method = self.frame_ref().method;

        let reason = if !self.announced_entry {
            self.announced_entry = true;
            crate::debug::StopReason::Entry
        } else if self.breakpoints.contains(method, il_offset) {
            crate::debug::StopReason::Breakpoint
        } else if self.stepping.is_some_and(|s| s.satisfied_at(depth)) {
            crate::debug::StopReason::Step
        } else {
            return;
        };

        self.debug_stop(reason, il_offset, depth);
    }

    /// Describes the stopped program, asks the debugger, and records what it
    /// said to do next.
    fn debug_stop(&mut self, reason: crate::debug::StopReason, il_offset: u32, depth: usize) {
        // The debugger is moved out for the call. It needs the frames and the
        // breakpoints at the same time, and they are different fields of the
        // same struct — taking the debugger out is what lets the borrow
        // checker see that, rather than a lock or a copy.
        let Some(mut debugger) = self.debugger.take() else { return };

        let frames: Vec<crate::debug::DebugFrame<'_>> = self
            .frames
            .iter()
            .enumerate()
            .rev()
            .map(|(index, frame)| {
                // The innermost frame is stopped *before* `il_offset`; every
                // caller is stopped at the call it is in the middle of, whose
                // offset is the instruction its pc last passed.
                let offset = if index + 1 == depth {
                    il_offset
                } else {
                    frame
                        .code
                        .instructions
                        .get(frame.pc.saturating_sub(1))
                        .map(|i| i.offset)
                        .unwrap_or(0)
                };
                crate::debug::DebugFrame {
                    method: frame.method,
                    method_name: &self.loader.registry.method(frame.method).qualified_name,
                    il_offset: offset,
                    arguments: &frame.args,
                    locals: &frame.locals,
                    stack: &frame.stack,
                }
            })
            .collect();

        let mut control = crate::debug::DebugControl { breakpoints: &mut self.breakpoints };
        let command = debugger.stopped(reason, &frames, &mut control);
        drop(frames);

        self.stepping = match command {
            crate::debug::DebugCommand::Run => None,
            crate::debug::DebugCommand::StepInto => Some(crate::debug::Stepping::Into),
            crate::debug::DebugCommand::StepOver => Some(crate::debug::Stepping::Over(depth)),
            crate::debug::DebugCommand::StepOut => Some(crate::debug::Stepping::Out(depth)),
            crate::debug::DebugCommand::Detach => None,
        };
        // Detaching drops the debugger rather than leaving it attached and
        // ignored, so the per-instruction branch goes back to costing nothing.
        if command != crate::debug::DebugCommand::Detach {
            self.debugger = Some(debugger);
        }
    }

    /// Attaches a debugger. The program stops before its first instruction.
    /// Lets the device service look at the running program.
    ///
    /// Cold and never inlined, for the reason the debug and profile hooks are:
    /// without it the branch and its registers get hoisted into the loop that
    /// runs every instruction and cost something even when nothing is
    /// attached.
    ///
    /// The service is **taken out and put back** rather than borrowed, because
    /// it needs `&mut Interpreter` — the canvas, the board, the statistics —
    /// and it cannot borrow the interpreter while living inside it. Taking it
    /// also means a service that somehow re-entered here would find nothing to
    /// call rather than recursing.
    #[cold]
    #[inline(never)]
    fn device_poll(&mut self) {
        let Some(mut service) = self.device.take() else { return };
        service.poll(self);
        self.device = Some(service);
    }

    pub fn attach_debugger(&mut self, debugger: crate::debug::BoxedDebugger) {
        self.debugger = Some(debugger);
        self.announced_entry = false;
        self.stepping = None;
    }

    fn run_until(&mut self, base_depth: usize) -> ExecResult<Option<Value>> {
        let mut result = None;

        while self.frames.len() > base_depth {
            if let Some(budget) = self.limits.max_instructions {
                if self.stats.instructions_executed >= budget {
                    return Err(ExecutionError::InstructionBudgetExceeded(budget));
                }
            }

            // Between instructions, where every live value is rooted and no
            // frame is mid-update -- the same property that makes this a safe
            // point for collection makes it the only safe place to abandon a
            // program from outside it.
            if let Some(cancel) = self.cancel.as_ref() {
                if cancel.load(core::sync::atomic::Ordering::Relaxed) {
                    return Err(ExecutionError::Cancelled);
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
        let pc = self.frames[top].pc;
        // Borrow the instruction rather than the method: cloning the `Arc`
        // here was an atomic increment and decrement on every instruction.
        let Some(instruction) = self.frames[top].code.instructions.get(pc).cloned() else {
            // Falling off the end without `ret` is invalid IL; return void
            // rather than spinning.
            return self.do_return(None);
        };

        // The debug hook, before the instruction rather than after: a
        // breakpoint means "stop *at* this", and a debugger that saw it having
        // already run could not step into the call it makes.
        if self.debugger.is_some() {
            self.debug_poll(instruction.offset);
        }
        // The profiler's own check is a decrement and a compare; the clock is
        // only read when that says an interval has passed.
        if self.profiler.is_some() {
            self.profile_poll();
        }
        // The device service is polled on the same principle: a check that
        // costs a branch when nothing is attached, and a cold call when
        // something is.
        if self.device.is_some() {
            self.device_poll();
        }

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

    /// A vector for a frame's arguments, locals or evaluation stack, reused
    /// from a returned frame when one is spare: a call used to cost three
    /// allocations before its first instruction ran.
    #[inline]
    pub(crate) fn frame_vec(&mut self) -> Vec<Value> {
        self.vec_pool.pop().unwrap_or_else(|| Vec::with_capacity(8))
    }

    #[inline]
    fn recycle(&mut self, mut v: Vec<Value>) {
        if self.vec_pool.len() < 256 && v.capacity() <= 256 {
            v.clear();
            self.vec_pool.push(v);
        }
    }

    fn do_return(&mut self, value: Option<Value>) -> ExecResult<StepOutcome> {
        let finished = self.frames.pop();
        let below_floor = self.frames.len() <= self.frame_floor;
        // A `.ctor` returns void, but `newobj` must leave the new instance on
        // the caller's stack.
        let value = value.or_else(|| match &finished {
            Some(frame) => match (frame.pending_newobj, frame.pending_newobj_is_cell) {
                // A value type was constructed into a temporary cell; hand the
                // caller the constructed value, not the cell.
                (Some(cell), true) => {
                    let v = self
                        .heap
                        .with_mut::<ClrObject, _>(cell, |o| o.fields.first_mut().map(|f| core::mem::replace(f, Value::Null)))
                        .flatten();
                    // Nothing can hold the cell now — C# cannot let `this`
                    // escape a struct constructor — so it serves the next one.
                    if self.struct_cells.len() < 64 {
                        self.struct_cells.push(cell);
                    }
                    v
                }
                (Some(handle), false) => Some(Value::Obj(handle)),
                _ => None,
            },
            None => None,
        });
        if let Some(frame) = finished {
            self.recycle(frame.args);
            self.recycle(frame.locals);
            self.recycle(frame.stack);
        }
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

/// A service polled from the interpreter's instruction loop.
///
/// Given the whole [`Interpreter`] because everything it is asked about lives
/// there: the framebuffer for a screenshot, the statistics for a performance
/// reading, the board for the UART it answers on and the flash it stores
/// applications in.
///
/// It runs *between* instructions, where every live value is rooted and no
/// frame is mid-update — the same property that makes that point safe for a
/// collection. It must not block: on these chips this is the same thread the
/// program runs on, so time spent here is time the program is not running.
pub trait DevicePoll: Send {
    fn poll(&mut self, interp: &mut Interpreter);

    /// The service as its own type again.
    ///
    /// A firmware hands its service to the interpreter for the duration of one
    /// program and wants it back afterwards -- the log, the uptime and the
    /// application list belong to the *board*, and starting the next program
    /// must not look like a reboot to whoever is watching. The same
    /// `as_any`-shaped escape hatch [`rustclr_gc::GcObject`] uses, for the
    /// same reason: this trait is deliberately narrow, and downcasting is
    /// cheaper than widening it with methods only one implementor wants.
    fn into_any(self: alloc::boxed::Box<Self>) -> alloc::boxed::Box<dyn core::any::Any>;
}
