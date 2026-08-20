//! `System.Object`, `System.Environment`, `System.GC`, `System.Array`,
//! diagnostics and time.

use crate::support::*;
use rustclr_core::{ClrArray, ClrException, Interpreter, Value};
use rustclr_gc::Handle;

#[allow(unused_imports)]
use crate::prelude::*;

pub fn register(interp: &mut Interpreter) {
    register_object(interp);
    register_compiler_services(interp);
    register_exceptions(interp);
    register_environment(interp);
    register_gc(interp);
    register_array(interp);
    register_time(interp);
    register_random(interp);
}

fn register_object(interp: &mut Interpreter) {
    // Every constructor chain ends here.
    interp.register_native("System.Object::.ctor()", |_i, _a| Ok(None));
    interp.register_native("System.ValueType::.ctor()", |_i, _a| Ok(None));

    interp.register_native("System.Object::ToString()", |i, a| {
        let v = arg(i, a, 0)?;
        let s = display(i, &v);
        Ok(Some(string_value(i, &s)))
    });
    interp.register_native("System.Object::GetHashCode()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        Ok(Some(Value::I32(h.to_bits() as i32)))
    });
    interp.register_native("System.Object::Equals(object)", |i, a| {
        let x = arg_handle(i, a, 0)?;
        let y = arg_handle(i, a, 1)?;
        Ok(Some(Value::I32((x == y) as i32)))
    });
    interp.register_native("System.Object::ReferenceEquals(object,object)", |i, a| {
        let x = arg_handle(i, a, 0)?;
        let y = arg_handle(i, a, 1)?;
        Ok(Some(Value::I32((x == y) as i32)))
    });
    interp.register_native("System.Object::GetType()", |i, a| {
        let value = arg(i, a, 0)?;
        // A boxed value reports the type it holds, not `System.Object`.
        let type_id = match &value {
            Value::Obj(h) if !h.is_null() => i.type_of(*h),
            Value::I32(_) => Some(i.loader.primitive_type(rustclr_core::Primitive::Int32)),
            Value::I64(_) => Some(i.loader.primitive_type(rustclr_core::Primitive::Int64)),
            Value::F(_) => Some(i.loader.primitive_type(rustclr_core::Primitive::Double)),
            Value::Struct(s) => Some(s.type_id),
            _ => return Err(rustclr_core::ExecutionError::null_reference()),
        };
        let Some(type_id) = type_id else {
            return Err(rustclr_core::ExecutionError::null_reference());
        };
        Ok(Some(Value::Obj(i.type_object(type_id))))
    });
}

/// `System.Runtime.CompilerServices.RuntimeHelpers`.
fn register_compiler_services(interp: &mut Interpreter) {
    // Roslyn compiles `new int[] { 1, 2, 3 }` into an empty array plus a call
    // to `InitializeArray`, with the element bytes stored as a field RVA in the
    // image. The parameter types are framework classes, so the arity key is the
    // one that binds reliably here.
    interp.register_native(
        "System.Runtime.CompilerServices.RuntimeHelpers::InitializeArray/2",
        initialize_array,
    );
    interp.register_native("System.Runtime.CompilerServices.RuntimeHelpers::RunClassConstructor/1", |_i, _a| {
        Ok(None)
    });
    interp.register_native(
        "System.Runtime.CompilerServices.RuntimeHelpers::get_OffsetToStringData()",
        |_i, _a| Ok(Some(Value::I32(12))),
    );
    // A record's generated `PrintMembers` calls this before recursing. It is a
    // guard, not an operation: on .NET it throws if the native stack is nearly
    // exhausted. RustCLR's interpreter has no native stack to exhaust — managed
    // recursion is bounded by an explicit frame budget that raises
    // `StackOverflowException` — so the guarantee already holds and there is
    // nothing to do here.
    interp.register_native(
        "System.Runtime.CompilerServices.RuntimeHelpers::EnsureSufficientExecutionStack()",
        |_i, _a| Ok(None),
    );
}

/// Copies a field's RVA blob into an array, honouring the element width.
fn initialize_array(
    interp: &mut Interpreter,
    args: &[Value],
) -> rustclr_core::ExecResult<Option<Value>> {
    let array = arg_handle(interp, args, 0)?;
    let raw_token = arg(interp, args, 1)?.as_i64().unwrap_or(0) as u32;
    let token = rustclr_core::metadata::Token(raw_token);

    let Some(assembly) = interp.current_assembly() else {
        return Ok(None);
    };
    let Some(data) = interp
        .loader
        .field_initial_data(assembly, token)
        .map(|d| d.to_vec())
    else {
        // No blob means the array is already zero-initialised, which is the
        // correct result for `new int[3]`.
        return Ok(None);
    };

    let Some(a) = interp.heap.get_as_mut::<ClrArray>(array) else {
        return Ok(None);
    };
    copy_initial_data(&mut a.storage, &data);
    Ok(None)
}

/// Writes little-endian image bytes into typed array storage.
fn copy_initial_data(storage: &mut rustclr_core::ArrayStorage, data: &[u8]) {
    use rustclr_core::ArrayStorage as S;

    macro_rules! fill {
        ($vec:expr, $width:expr, $decode:expr) => {{
            let width: usize = $width;
            let decode: fn(&[u8]) -> _ = $decode;
            for (index, slot) in $vec.iter_mut().enumerate() {
                let start = index * width;
                match data.get(start..start + width) {
                    Some(chunk) => *slot = decode(chunk),
                    // A short blob leaves the remaining elements at zero.
                    None => break,
                }
            }
        }};
    }

    match storage {
        S::Bool(v) => fill!(v, 1, |c| c[0] != 0),
        S::I8(v) => fill!(v, 1, |c| c[0] as i8),
        S::U8(v) => fill!(v, 1, |c| c[0]),
        S::I16(v) => fill!(v, 2, |c| i16::from_le_bytes([c[0], c[1]])),
        S::U16(v) | S::Char(v) => fill!(v, 2, |c| u16::from_le_bytes([c[0], c[1]])),
        S::I32(v) => fill!(v, 4, |c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        S::U32(v) => fill!(v, 4, |c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        S::I64(v) => fill!(v, 8, |c| i64::from_le_bytes([
            c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]
        ])),
        S::U64(v) => fill!(v, 8, |c| u64::from_le_bytes([
            c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]
        ])),
        S::F32(v) => fill!(v, 4, |c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        S::F64(v) => fill!(v, 8, |c| f64::from_le_bytes([
            c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]
        ])),
        // Reference and boxed element types are never initialised this way.
        S::Refs(_) | S::Values(_) => {}
    }
}

fn register_exceptions(interp: &mut Interpreter) {
    // Exception constructors, in the shapes user code actually calls.
    interp.register_native("System.Exception::.ctor()", |_i, _a| Ok(None));
    interp.register_native("System.Exception::.ctor(string)", |i, a| {
        set_exception_message(i, a)
    });
    interp.register_native("System.Exception::get_Message()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let message = exception_message(i, h);
        Ok(Some(string_value(i, &message)))
    });
    interp.register_native("System.Exception::get_StackTrace()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let trace = i
            .heap
            .get_as::<ClrException>(h)
            .map(|e| e.stack_trace.join("\n"))
            .unwrap_or_default();
        Ok(Some(string_value(i, &trace)))
    });
    interp.register_native("System.Exception::ToString()", |i, a| {
        let v = arg(i, a, 0)?;
        let s = display(i, &v);
        Ok(Some(string_value(i, &s)))
    });

    for name in [
        "System.ArgumentException",
        "System.ArgumentNullException",
        "System.ArgumentOutOfRangeException",
        "System.InvalidOperationException",
        "System.NotSupportedException",
        "System.NotImplementedException",
        "System.FormatException",
        "System.ApplicationException",
        "System.SystemException",
    ] {
        let ctor0 = Box::leak(format!("{name}::.ctor()").into_boxed_str()) as &str;
        interp.register_native(ctor0, |_i, _a| Ok(None));
        let ctor1 = Box::leak(format!("{name}::.ctor(string)").into_boxed_str()) as &str;
        interp.register_native(ctor1, |i, a| set_exception_message(i, a));
        // `(message, innerException)` and `(paramName, message)` both carry the
        // message first, which is the part a program reads back.
        let ctor2 = Box::leak(format!("{name}::.ctor/2").into_boxed_str()) as &str;
        interp.register_native(ctor2, |i, a| set_exception_message(i, a));
    }
}

/// Reads an exception's message, whichever shape the instance has.
///
/// The runtime raises its own errors as `ClrException`, but `new
/// SomeException("…")` goes through `newobj` like any other class and produces
/// a plain object whose first field holds the message. Reading only the first
/// shape is what made `e.Message` come back empty for every exception a program
/// threw itself.
pub fn exception_message(interp: &Interpreter, handle: Handle) -> String {
    if let Some(e) = interp.heap.get_as::<ClrException>(handle) {
        return e.message.clone();
    }
    match interp.heap.get_as::<rustclr_core::ClrObject>(handle) {
        Some(o) => match o.fields.first() {
            Some(Value::Obj(text)) => read_string(interp, *text),
            _ => String::new(),
        },
        None => String::new(),
    }
}

/// Records the message on a freshly constructed exception object.
///
/// User-declared exception types are plain objects, so the message is stored on
/// the first field when the runtime's own `ClrException` is not in play.
fn set_exception_message(
    interp: &mut Interpreter,
    args: &[Value],
) -> rustclr_core::ExecResult<Option<Value>> {
    let this = arg_handle(interp, args, 0)?;
    let message = arg_string_or_empty(interp, args, 1)?;
    if let Some(e) = interp.heap.get_as_mut::<ClrException>(this) {
        e.message = message;
        return Ok(None);
    }
    let handle = interp.alloc_string(&message);
    if let Some(o) = interp.heap.get_as_mut::<rustclr_core::ClrObject>(this) {
        if o.fields.is_empty() {
            o.fields.push(Value::Obj(handle));
        } else {
            o.fields[0] = Value::Obj(handle);
        }
    }
    Ok(None)
}

fn register_environment(interp: &mut Interpreter) {
    interp.register_native("System.Environment::get_NewLine()", |i, _a| {
        Ok(Some(string_value(i, NEWLINE)))
    });
    interp.register_native("System.Environment::Exit(int)", |i, a| {
        let code = arg_i32(i, a, 0)?;
        i.request_exit(code);
        i.host.exit(code);
        Ok(None)
    });
    interp.register_native("System.Environment::get_TickCount()", |i, _a| {
        Ok(Some(Value::I32(i.host.monotonic_millis() as i32)))
    });
    // An iterator method's state machine records the thread that created it, so
    // that enumerating from another thread hands out a fresh instance. Managed
    // threads are serialised here (see `capabilities`), so there is exactly one
    // id — and returning it is what makes `yield return` work.
    interp.register_native("System.Environment::get_CurrentManagedThreadId()", |_i, _a| {
        Ok(Some(Value::I32(1)))
    });
    interp.register_native("System.Environment::get_ProcessorCount()", |_i, _a| {
        #[cfg(feature = "std")]
        let cores = std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(1);
        // A microcontroller has one core and no way to ask. Reporting 1 is the
        // truth here, not a fallback.
        #[cfg(not(feature = "std"))]
        let cores = 1;
        Ok(Some(Value::I32(cores)))
    });
    interp.register_native("System.Environment::GetCommandLineArgs()", |i, _a| {
        let args: Vec<String> = i.host.args().to_vec();
        let array = string_array(i, &args);
        Ok(Some(Value::Obj(array)))
    });
    interp.register_native("System.Environment::get_MachineName()", |i, _a| {
        Ok(Some(string_value(i, "rustclr")))
    });
}

fn register_gc(interp: &mut Interpreter) {
    interp.register_native("System.GC::Collect()", |i, _a| {
        i.force_collect();
        Ok(None)
    });
    interp.register_native("System.GC::GetTotalMemory(bool)", |i, a| {
        if arg_bool(i, a, 0)? {
            i.force_collect();
        }
        Ok(Some(Value::I64(i.heap.live_bytes() as i64)))
    });
    interp.register_native("System.GC::get_MaxGeneration()", |_i, _a| {
        // The default collector is non-generational.
        Ok(Some(Value::I32(0)))
    });
    interp.register_native("System.GC::SuppressFinalize(object)", |_i, _a| Ok(None));
}

fn register_array(interp: &mut Interpreter) {
    interp.register_native("System.Array::get_Length()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let len = i.heap.get_as::<ClrArray>(h).map(|x| x.len()).unwrap_or(0);
        Ok(Some(Value::I32(len as i32)))
    });
    interp.register_native("System.Array::get_Rank()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let rank = i.heap.get_as::<ClrArray>(h).map(|x| x.dimensions.len()).unwrap_or(1);
        Ok(Some(Value::I32(rank as i32)))
    });
    interp.register_native("System.Array::GetLength(int)", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let dim = arg_i32(i, a, 1)?.max(0) as usize;
        let len = i
            .heap
            .get_as::<ClrArray>(h)
            .and_then(|x| x.dimensions.get(dim).copied())
            .unwrap_or(0);
        Ok(Some(Value::I32(len as i32)))
    });
    interp.register_native("System.Array::Clear(#0,int,int)", |_i, _a| Ok(None));
}

fn register_time(interp: &mut Interpreter) {
    interp.register_native("System.DateTime::get_Now()", |i, _a| {
        Ok(Some(Value::I64(i.host.wall_clock_millis())))
    });
    interp.register_native("System.DateTime::get_UtcNow()", |i, _a| {
        Ok(Some(Value::I64(i.host.wall_clock_millis())))
    });
    interp.register_native("System.DateTime::get_Ticks()", |i, a| {
        // .NET ticks are 100ns units; the host clock is milliseconds.
        Ok(Some(Value::I64(arg_i64(i, a, 0)? * 10_000)))
    });

    interp.register_native("System.Diagnostics.Stopwatch::StartNew()", |i, _a| {
        let start = i.host.monotonic_millis();
        Ok(Some(Value::I64(start as i64)))
    });
    interp.register_native("System.Diagnostics.Stopwatch::get_ElapsedMilliseconds()", |i, a| {
        let start = arg_i64(i, a, 0)?;
        let now = i.host.monotonic_millis() as i64;
        Ok(Some(Value::I64((now - start).max(0))))
    });

    interp.register_native("System.Threading.Thread::Sleep(int)", |i, a| {
        let ms = arg_i32(i, a, 0)?.max(0) as u64;
        sleep_millis(i, ms);
        Ok(None)
    });
}

fn register_random(interp: &mut Interpreter) {
    // A deterministic xorshift generator seeded from the object identity, so a
    // program's random sequence is reproducible across runs of the same build.
    interp.register_native("System.Random::.ctor()", |_i, _a| Ok(None));
    interp.register_native("System.Random::.ctor(int)", |_i, _a| Ok(None));
    interp.register_native("System.Random::Next()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        Ok(Some(Value::I32((next_random(h.to_bits()) >> 1) as i32)))
    });
    interp.register_native("System.Random::Next(int)", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let bound = arg_i32(i, a, 1)?.max(1);
        Ok(Some(Value::I32((next_random(h.to_bits()) % bound as u64) as i32)))
    });
    interp.register_native("System.Random::Next(int,int)", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let lo = arg_i32(i, a, 1)?;
        let hi = arg_i32(i, a, 2)?.max(lo + 1);
        let span = (hi - lo) as u64;
        Ok(Some(Value::I32(lo + (next_random(h.to_bits()) % span) as i32)))
    });
    interp.register_native("System.Random::NextDouble()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let v = next_random(h.to_bits()) as f64 / u32::MAX as f64;
        Ok(Some(Value::F(crate::fmath::fract(v))))
    });
}

/// One step of a xorshift64* generator, keyed by the instance handle.
///
/// State lives in a process-wide counter rather than on the instance, so two
/// `Random` objects share a stream. That is a simplification, documented here
/// rather than hidden: it keeps sequences reproducible without adding a heap
/// object kind purely for RNG state.
fn next_random(seed: u64) -> u64 {
    let mut x = load_random_state() ^ seed.rotate_left(17);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    store_random_state(x);
    x >> 33
}

const RANDOM_SEED: u64 = 0x2545F4914F6CDD1D;

#[cfg(feature = "std")]
static RANDOM_STATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(RANDOM_SEED);

#[cfg(feature = "std")]
fn load_random_state() -> u64 {
    RANDOM_STATE.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(feature = "std")]
fn store_random_state(x: u64) {
    RANDOM_STATE.store(x, std::sync::atomic::Ordering::Relaxed);
}

// RISC-V `imc` — the ESP32-C3's core — has no atomics extension at all, so
// `AtomicU64` does not exist there. A plain static is sound in its place for
// the same reason the interpreter needs no locks on a chip: there is one
// thread. Accessed through raw pointers rather than references, which is what
// keeps this from being a `static_mut_refs` violation.
#[cfg(not(feature = "std"))]
static mut RANDOM_STATE: u64 = RANDOM_SEED;

#[cfg(not(feature = "std"))]
fn load_random_state() -> u64 {
    unsafe { core::ptr::read(&raw const RANDOM_STATE) }
}

#[cfg(not(feature = "std"))]
fn store_random_state(x: u64) {
    unsafe { core::ptr::write(&raw mut RANDOM_STATE, x) }
}
