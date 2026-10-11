//! `System.Object`, `System.Environment`, `System.GC`, `System.Array`,
//! diagnostics and time.

use crate::collections::{field, set_field};
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
    // A boxed value (an enum, a struct without its own overrides) hashes and
    // compares by what it holds, as ValueType and Enum do on .NET; anything
    // else by identity.
    interp.register_native("System.Object::GetHashCode()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let boxed = i.heap.with::<rustclr_core::ClrBox, _>(h, |b| b.value.clone());
        Ok(Some(Value::I32(match boxed {
            Some(Value::I32(v)) => v,
            Some(Value::I64(v)) | Some(Value::NativeInt(v)) => (v as i32) ^ ((v >> 32) as i32),
            Some(v) => crate::collections::value_hash(i, &v) as i32,
            None => h.to_bits() as i32,
        })))
    });
    interp.register_native("System.Object::Equals(object)", |i, a| {
        let x = arg_handle(i, a, 0)?;
        let y = arg_handle(i, a, 1)?;
        if x == y {
            return Ok(Some(Value::I32(1)));
        }
        let left = i.heap.with::<rustclr_core::ClrBox, _>(x, |b| (b.type_id, b.value.clone()));
        let right = i.heap.with::<rustclr_core::ClrBox, _>(y, |b| (b.type_id, b.value.clone()));
        Ok(Some(Value::I32(match (left, right) {
            (Some((tx, vx)), Some((ty, vy))) => (tx == ty && crate::collections::values_equal(i, &vx, &vy)) as i32,
            _ => 0,
        })))
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

    // An enum array keeps its elements as stack values, so the blob is read
    // at the width and sign of the enum's underlying type.
    let element = interp.heap.with::<ClrArray, _>(array, |a| a.element_type);
    let underlying = element.and_then(|t| enum_underlying(interp, t));
    interp.heap.with_mut::<ClrArray, _>(array, |a| match (&mut a.storage, underlying) {
        (rustclr_core::ArrayStorage::Values(values), Some(p)) => copy_enum_data(values, p, &data),
        (storage, _) => copy_initial_data(storage, &data),
    });
    Ok(None)
}

/// The integer type an enum is stored as: recorded for the framework's enums,
/// read from the `value__` field's signature for the program's own.
fn enum_underlying(interp: &Interpreter, ty: rustclr_core::types::TypeId) -> Option<rustclr_core::types::Primitive> {
    use rustclr_core::metadata::TypeSig as T;
    use rustclr_core::types::{Primitive as P, TypeKind};
    let registry = &interp.loader.registry;
    let runtime = registry.ty(ty);
    if runtime.underlying.is_some() || runtime.kind != TypeKind::Enum {
        return runtime.underlying;
    }
    let field = runtime.instance_fields.first()?;
    Some(match registry.field(*field).signature.unwrap_modifiers() {
        T::I1 => P::SByte,
        T::U1 => P::Byte,
        T::Boolean => P::Boolean,
        T::I2 => P::Int16,
        T::U2 => P::UInt16,
        T::Char => P::Char,
        T::U4 => P::UInt32,
        T::I8 => P::Int64,
        T::U8 => P::UInt64,
        _ => P::Int32,
    })
}

/// Writes little-endian image bytes into an enum array's value slots.
fn copy_enum_data(values: &mut [Value], underlying: rustclr_core::types::Primitive, data: &[u8]) {
    use rustclr_core::types::Primitive as P;
    let width = match underlying {
        P::SByte | P::Byte | P::Boolean => 1,
        P::Int16 | P::UInt16 | P::Char => 2,
        P::Int64 | P::UInt64 => 8,
        _ => 4,
    };
    for (index, slot) in values.iter_mut().enumerate() {
        let Some(c) = data.get(index * width..index * width + width) else { break };
        *slot = match underlying {
            P::SByte => Value::I32(c[0] as i8 as i32),
            P::Byte | P::Boolean => Value::I32(c[0] as i32),
            P::Int16 => Value::I32(i16::from_le_bytes([c[0], c[1]]) as i32),
            P::UInt16 | P::Char => Value::I32(u16::from_le_bytes([c[0], c[1]]) as i32),
            P::Int64 | P::UInt64 => Value::I64(i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])),
            _ => Value::I32(i32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        };
    }
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
        // Reference elements are never initialised this way; enum elements
        // are handled by `copy_enum_data`.
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
            .heap.with::<ClrException, _>(h, |e| e.stack_trace.join("\n"))
            .unwrap_or_default();
        Ok(Some(string_value(i, &trace)))
    });
    // "Type: message", then the stack where it was thrown, as .NET prints it.
    // A page logging an unhandled exception is often all there is to go on.
    interp.register_native("System.Exception::ToString()", |i, a| {
        let v = arg(i, a, 0)?;
        let mut s = display(i, &v);
        if let Some(h) = v.as_handle().filter(|h| !h.is_null()) {
            let trace = i.heap.with::<ClrException, _>(h, |e| e.stack_trace.join("\n")).unwrap_or_default();
            if !trace.is_empty() {
                s.push('\n');
                s.push_str(&trace);
            }
        }
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
    if let Some(message) = interp.heap.with::<ClrException, _>(handle, |e| e.message.clone()) {
        return message;
    }
    let first = interp
        .heap
        .with::<rustclr_core::ClrObject, _>(handle, |o| o.fields.first().cloned());
    match first {
        Some(Some(Value::Obj(text))) => read_string(interp, text),
        _ => String::new(),
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
    // `message` is moved into the exception when there is one, so it is
    // handed to the closure and taken back if the handle names something else.
    let mut message = Some(message);
    let stored = interp
        .heap
        .with_mut::<ClrException, _>(this, |e| e.message = message.take().unwrap_or_default());
    if stored.is_some() {
        return Ok(None);
    }
    let handle = interp.alloc_string(&message.unwrap_or_default());
    interp.heap.with_mut::<rustclr_core::ClrObject, _>(this, |o| {
        if o.fields.is_empty() {
            o.fields.push(Value::Obj(handle));
        } else {
            o.fields[0] = Value::Obj(handle);
        }
    });
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
    interp.register_native("System.Environment::get_TickCount64()", |i, _a| {
        Ok(Some(Value::I64(i.host.monotonic_millis() as i64)))
    });
    // An iterator method's state machine records the thread that created it, so
    // that enumerating it from another thread hands out a fresh instance. Now
    // that `Thread.Start` really spawns, every thread needs an id of its own:
    // returning 1 everywhere would make two threads enumerating the same
    // iterator share one state machine and interleave through it.
    //
    // The numbering is arbitrary, as it is on .NET — the only guarantees are
    // that it is stable for a thread and distinct between them.
    interp.register_native("System.Environment::get_CurrentManagedThreadId()", |_i, _a| {
        Ok(Some(Value::I32(managed_thread_id())))
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

    // -- diagnostics ---------------------------------------------------------
    //
    // These are the counters a program reads about itself. Every one of them is
    // answered from the same numbers `rustnet profile` reports, so a program's
    // own view and the profiler's cannot drift apart.

    // The generation argument is accepted and ignored: the default collector is
    // non-generational, `MaxGeneration` says 0, and answering the same count for
    // every generation is what a single-generation collector means. A program
    // asking about generation 2 gets a true answer about the only heap there is.
    interp.register_native("System.GC::CollectionCount(int)", |i, _a| {
        Ok(Some(Value::I32(i.heap.stats().collections as i32)))
    });

    // Cumulative, never decreasing — the difference between two readings is
    // what a program actually uses this for. `precise` changes nothing here:
    // the count is exact already, because every allocation goes through the
    // interpreter rather than a thread-local buffer that has to be flushed.
    interp.register_native("System.GC::GetTotalAllocatedBytes(bool)", |i, _a| {
        Ok(Some(Value::I64(i.heap.stats().total_bytes_allocated as i64)))
    });
    interp.register_native("System.GC::GetTotalAllocatedBytes()", |i, _a| {
        Ok(Some(Value::I64(i.heap.stats().total_bytes_allocated as i64)))
    });

    // One heap, shared by every thread, and no per-thread allocation context —
    // so this is the process total. Stated in `docs/limitations.md` rather than
    // silently returning a plausible smaller number.
    interp.register_native("System.GC::GetAllocatedBytesForCurrentThread()", |i, _a| {
        Ok(Some(Value::I64(i.heap.stats().total_bytes_allocated as i64)))
    });
}

fn register_array(interp: &mut Interpreter) {
    interp.register_native("System.Array::get_Length()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let len = i.heap.with::<ClrArray, _>(h, |x| x.len()).unwrap_or(0);
        Ok(Some(Value::I32(len as i32)))
    });
    interp.register_native("System.Array::get_Rank()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let rank = i.heap.with::<ClrArray, _>(h, |x| x.dimensions.len()).unwrap_or(1);
        Ok(Some(Value::I32(rank as i32)))
    });
    interp.register_native("System.Array::GetLength(int)", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let dim = arg_i32(i, a, 1)?.max(0) as usize;
        let len = i
            .heap.with::<ClrArray, _>(h, |x| x.dimensions.get(dim).copied()).flatten()
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

    // A `Stopwatch` is an object with two slots: when the current run began,
    // and how much time earlier runs added up to. `_start` is -1 while stopped.
    //
    // It used to be a bare number — the moment it was started — which reads
    // `ElapsedMilliseconds` correctly and cannot express `Restart`, because
    // `this` for a class arrives by value and there is nothing to write back to.
    interp.register_native("System.Diagnostics.Stopwatch::StartNew()", |i, _a| {
        let watch = new_stopwatch(i)?;
        let now = i.host.monotonic_millis() as i64;
        set_field(i, watch, WATCH_START, Value::I64(now));
        Ok(Some(Value::Obj(watch)))
    });
    for ctor in ["System.Diagnostics.Stopwatch::.ctor()", "System.Diagnostics.Stopwatch::.ctor/0"] {
        interp.register_native(ctor, |i, a| {
            // `newobj` made the instance; this only has to initialise it.
            if let Some(watch) = arg(i, a, 0)?.as_handle().filter(|h| !h.is_null()) {
                set_field(i, watch, WATCH_START, Value::I64(-1));
                set_field(i, watch, WATCH_ELAPSED, Value::I64(0));
            }
            Ok(None)
        });
    }
    interp.register_native("System.Diagnostics.Stopwatch::get_ElapsedMilliseconds()", |i, a| {
        let watch = arg(i, a, 0)?.as_handle().unwrap_or(rustclr_gc::Handle::NULL);
        Ok(Some(Value::I64(elapsed_millis(i, watch))))
    });
    interp.register_native("System.Diagnostics.Stopwatch::get_IsRunning()", |i, a| {
        let watch = arg(i, a, 0)?.as_handle().unwrap_or(rustclr_gc::Handle::NULL);
        let running = field(i, watch, WATCH_START).as_i64().unwrap_or(-1) >= 0;
        Ok(Some(Value::I32(running as i32)))
    });
    interp.register_native("System.Diagnostics.Stopwatch::Start()", |i, a| {
        let watch = arg(i, a, 0)?.as_handle().unwrap_or(rustclr_gc::Handle::NULL);
        if field(i, watch, WATCH_START).as_i64().unwrap_or(-1) < 0 {
            let now = i.host.monotonic_millis() as i64;
            set_field(i, watch, WATCH_START, Value::I64(now));
        }
        Ok(None)
    });
    interp.register_native("System.Diagnostics.Stopwatch::Stop()", |i, a| {
        let watch = arg(i, a, 0)?.as_handle().unwrap_or(rustclr_gc::Handle::NULL);
        let start = field(i, watch, WATCH_START).as_i64().unwrap_or(-1);
        if start >= 0 {
            let now = i.host.monotonic_millis() as i64;
            let accumulated = field(i, watch, WATCH_ELAPSED).as_i64().unwrap_or(0);
            set_field(i, watch, WATCH_ELAPSED, Value::I64(accumulated + (now - start).max(0)));
            set_field(i, watch, WATCH_START, Value::I64(-1));
        }
        Ok(None)
    });
    interp.register_native("System.Diagnostics.Stopwatch::Reset()", |i, a| {
        let watch = arg(i, a, 0)?.as_handle().unwrap_or(rustclr_gc::Handle::NULL);
        set_field(i, watch, WATCH_START, Value::I64(-1));
        set_field(i, watch, WATCH_ELAPSED, Value::I64(0));
        Ok(None)
    });
    interp.register_native("System.Diagnostics.Stopwatch::Restart()", |i, a| {
        let watch = arg(i, a, 0)?.as_handle().unwrap_or(rustclr_gc::Handle::NULL);
        let now = i.host.monotonic_millis() as i64;
        set_field(i, watch, WATCH_START, Value::I64(now));
        set_field(i, watch, WATCH_ELAPSED, Value::I64(0));
        Ok(None)
    });

    interp.register_native("System.Threading.Thread::Sleep(int)", |i, a| {
        let ms = arg_i32(i, a, 0)?.max(0) as u64;
        sleep_millis(i, ms);
        Ok(None)
    });
}

fn register_random(interp: &mut Interpreter) {
    // `new Random(seed)` reproduces .NET's sequence exactly: the subtractive
    // generator .NET keeps for seeded instances. Programs depend on that for
    // reproducible simulations, procedural content and tests. `new Random()`
    // uses the same generator from a clock-derived seed, so every run differs,
    // as in .NET. Each instance keeps its own state in its first slot.
    for ctor in ["System.Random::.ctor()", "System.Random::.ctor/0"] {
        interp.register_native(ctor, |i, a| {
            let h = arg_handle(i, a, 0)?;
            let seed = clock_seed(i, h);
            init_random(i, h, seed);
            Ok(None)
        });
    }
    for ctor in ["System.Random::.ctor(int)", "System.Random::.ctor/1"] {
        interp.register_native(ctor, |i, a| {
            let h = arg_handle(i, a, 0)?;
            let seed = arg_i32(i, a, 1)?;
            init_random(i, h, seed);
            Ok(None)
        });
    }
    interp.register_native("System.Random::get_Shared()", |i, _| {
        let Some(ty) = i.loader.registry.find_type_by_name("System.Random") else { return Ok(Some(Value::Null)) };
        let h = i.alloc_object(ty);
        let seed = clock_seed(i, h);
        init_random(i, h, seed);
        Ok(Some(Value::Obj(h)))
    });
    interp.register_native("System.Random::Next()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        Ok(Some(Value::I32(with_prng(i, h, |p| p.internal_sample()))))
    });
    interp.register_native("System.Random::Next(int)", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let max = arg_i32(i, a, 1)?;
        if max < 0 {
            return Err(random_range("'maxValue' must be greater than zero."));
        }
        Ok(Some(Value::I32((with_prng(i, h, |p| p.sample()) * max as f64) as i32)))
    });
    interp.register_native("System.Random::Next(int,int)", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let (lo, hi) = (arg_i32(i, a, 1)?, arg_i32(i, a, 2)?);
        if lo > hi {
            return Err(random_range("'minValue' cannot be greater than maxValue."));
        }
        let range = hi as i64 - lo as i64;
        let v = if range <= i32::MAX as i64 {
            (with_prng(i, h, |p| p.sample()) * range as f64) as i32 + lo
        } else {
            ((with_prng(i, h, |p| p.large_range_sample()) * range as f64) as i64 + lo as i64) as i32
        };
        Ok(Some(Value::I32(v)))
    });
    interp.register_native("System.Random::NextDouble()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        Ok(Some(Value::F(with_prng(i, h, |p| p.sample()))))
    });
    interp.register_native("System.Random::NextSingle()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        Ok(Some(Value::F(with_prng(i, h, |p| p.sample()) as f32 as f64)))
    });
    for name in ["System.Random::NextInt64()", "System.Random::NextInt64(long)", "System.Random::NextInt64(long,long)"] {
        interp.register_native(name, |i, a| {
            let h = arg_handle(i, a, 0)?;
            let (lo, hi) = match a.len() {
                1 => (0i64, i64::MAX),
                2 => (0, arg_i64(i, a, 1)?),
                _ => (arg_i64(i, a, 1)?, arg_i64(i, a, 2)?),
            };
            if lo > hi {
                return Err(random_range("'minValue' cannot be greater than maxValue."));
            }
            let span = (hi as i128 - lo as i128) as f64;
            let v = lo as i128 + (with_prng(i, h, |p| p.large_range_sample()) * span) as i128;
            Ok(Some(Value::I64(v.clamp(lo as i128, (hi as i128 - 1).max(lo as i128)) as i64)))
        });
    }
    interp.register_native("System.Random::NextBytes(byte[])", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let buffer = arg_handle(i, a, 1)?;
        let len = i.heap.with::<ClrArray, _>(buffer, |x| x.len()).unwrap_or(0);
        let bytes: Vec<i32> = (0..len).map(|_| with_prng(i, h, |p| p.internal_sample()) % 256).collect();
        i.heap.with_mut::<ClrArray, _>(buffer, |x| {
            for (n, b) in bytes.iter().enumerate() {
                x.storage.set(n, &Value::I32(*b));
            }
        });
        Ok(None)
    });
}

fn random_range(message: &str) -> rustclr_core::ExecutionError {
    rustclr_core::ExecutionError::exception(rustclr_core::ClrExceptionKind::ArgumentOutOfRange, message)
}

/// .NET's seeded generator (Knuth's subtractive method, `Net5CompatSeedImpl`).
struct Prng<'a> {
    state: &'a mut [i32],
}

const MBIG: i32 = i32::MAX;
const MSEED: i32 = 161_803_398;
/// 56 seed slots, then `inext` and `inextp`.
const PRNG_SLOTS: usize = 58;

impl Prng<'_> {
    fn seed(state: &mut [i32], seed: i32) {
        let subtraction = if seed == i32::MIN { i32::MAX } else { seed.abs() };
        let mut mj = MSEED - subtraction;
        state[55] = mj;
        let mut mk = 1i32;
        let mut ii = 0usize;
        for _ in 1..55 {
            ii += 21;
            if ii >= 55 {
                ii -= 55;
            }
            state[ii] = mk;
            mk = mj.wrapping_sub(mk);
            if mk < 0 {
                mk = mk.wrapping_add(MBIG);
            }
            mj = state[ii];
        }
        for _ in 1..5 {
            for i in 1..56 {
                let mut n = i + 30;
                if n >= 55 {
                    n -= 55;
                }
                state[i] = state[i].wrapping_sub(state[1 + n]);
                if state[i] < 0 {
                    state[i] = state[i].wrapping_add(MBIG);
                }
            }
        }
        state[56] = 0;
        state[57] = 21;
    }

    fn internal_sample(&mut self) -> i32 {
        let s = &mut *self.state;
        let mut next = s[56] as usize + 1;
        if next >= 56 {
            next = 1;
        }
        let mut nextp = s[57] as usize + 1;
        if nextp >= 56 {
            nextp = 1;
        }
        let mut v = s[next].wrapping_sub(s[nextp]);
        if v == MBIG {
            v -= 1;
        }
        if v < 0 {
            v = v.wrapping_add(MBIG);
        }
        s[next] = v;
        s[56] = next as i32;
        s[57] = nextp as i32;
        v
    }

    fn sample(&mut self) -> f64 {
        self.internal_sample() as f64 * (1.0 / MBIG as f64)
    }

    fn large_range_sample(&mut self) -> f64 {
        let mut result = self.internal_sample();
        if self.internal_sample() % 2 == 0 {
            result = -result;
        }
        let d = result as f64 + (i32::MAX - 1) as f64;
        d / (2.0 * i32::MAX as f64 - 1.0)
    }
}

/// Gives the instance a state array seeded with `seed`.
fn init_random(i: &mut Interpreter, h: Handle, seed: i32) {
    let int_type = i.loader.primitive_type(rustclr_core::Primitive::Int32);
    let array = i.alloc_array(int_type, PRNG_SLOTS);
    i.heap.with_mut::<ClrArray, _>(array, |x| {
        if let rustclr_core::ArrayStorage::I32(v) = &mut x.storage {
            Prng::seed(v, seed);
        }
    });
    i.heap.with_mut::<rustclr_core::ClrObject, _>(h, |o| {
        if o.fields.is_empty() {
            o.fields.push(Value::Obj(array));
        } else {
            o.fields[0] = Value::Obj(array);
        }
    });
}

/// Runs one generator step on the instance's state, seeding it first if a
/// subclass constructor or reflection skipped `.ctor`.
fn with_prng<R>(i: &mut Interpreter, h: Handle, f: impl FnOnce(&mut Prng) -> R) -> R {
    let mut state = match field(i, h, 0) {
        Value::Obj(s) => s,
        _ => Handle::NULL,
    };
    if state.is_null() {
        let seed = clock_seed(i, h);
        init_random(i, h, seed);
        if let Value::Obj(s) = field(i, h, 0) {
            state = s;
        }
    }
    let mut f = Some(f);
    let mut out = None;
    i.heap.with_mut::<ClrArray, _>(state, |x| {
        if let rustclr_core::ArrayStorage::I32(v) = &mut x.storage {
            if let Some(f) = f.take() {
                out = Some(f(&mut Prng { state: v.as_mut_slice() }));
            }
        }
    });
    match out {
        Some(v) => v,
        None => {
            // No usable state (a foreign object): a throwaway generator.
            let mut v = [0i32; PRNG_SLOTS];
            Prng::seed(&mut v, 0);
            let f = f.take().expect("generator closure");
            f(&mut Prng { state: &mut v })
        }
    }
}

/// A seed that differs per run and per instance, for `new Random()`.
fn clock_seed(i: &mut Interpreter, h: Handle) -> i32 {
    let t = i.host.wall_clock_millis() as u64;
    let mixed = next_random(t ^ h.to_bits().rotate_left(29));
    (mixed as u32 & 0x7FFF_FFFF) as i32
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

/// A small, stable, per-thread number.
#[cfg(feature = "std")]
fn managed_thread_id() -> i32 {
    use core::sync::atomic::{AtomicI32, Ordering};
    static NEXT: AtomicI32 = AtomicI32::new(1);
    std::thread_local! {
        static ID: i32 = NEXT.fetch_add(1, Ordering::Relaxed);
    }
    ID.with(|id| *id)
}

/// One thread, one id.
#[cfg(not(feature = "std"))]
fn managed_thread_id() -> i32 {
    1
}

/// Slots of a `Stopwatch`.
const WATCH_START: usize = 0;
const WATCH_ELAPSED: usize = 1;

fn new_stopwatch(interp: &mut Interpreter) -> rustclr_core::ExecResult<rustclr_gc::Handle> {
    let Some(type_id) = interp
        .loader
        .registry
        .find_type_by_name("System.Diagnostics.Stopwatch")
    else {
        return Err(rustclr_core::ExecutionError::MissingImplementation(
            "Stopwatch is not registered".into(),
        ));
    };
    let watch = interp.alloc_object(type_id);
    set_field(interp, watch, WATCH_START, Value::I64(-1));
    set_field(interp, watch, WATCH_ELAPSED, Value::I64(0));
    Ok(watch)
}

/// Time on the clock: what earlier runs added up to, plus the current run.
fn elapsed_millis(interp: &mut Interpreter, watch: rustclr_gc::Handle) -> i64 {
    let accumulated = field(interp, watch, WATCH_ELAPSED).as_i64().unwrap_or(0);
    let start = field(interp, watch, WATCH_START).as_i64().unwrap_or(-1);
    if start < 0 {
        return accumulated;
    }
    let now = interp.host.monotonic_millis() as i64;
    accumulated + (now - start).max(0)
}
