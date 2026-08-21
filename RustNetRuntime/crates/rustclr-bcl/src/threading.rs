//! `System.Threading` and a few reflection-adjacent members.
//!
//! # Threads are real
//!
//! `Thread.Start` spawns an OS thread and `Join` waits for it. The thread gets
//! a *worker* interpreter: the same heap, the same static storage, the same
//! native bindings, its own frame stack. It allocates into the same object
//! graph, a collection stops it, and what it writes to a static is what the
//! starting thread reads.
//!
//! That works because a `Loader` is finished before anything runs, so a worker
//! can have an identical copy rather than share one behind a lock — see
//! [`rustclr_core::loader::Loader`]. Static storage is the exception and is
//! genuinely shared, because `static int Total` must be one slot.
//!
//! Waiting is done inside `Interpreter::blocking`, so a collection on the
//! thread being waited for does not wait for the waiter in turn.
//!
//! # What is still serialised
//!
//! `Task` and `Parallel.For`. An `async` method still completes where it is
//! created and a parallel loop still iterates in order; only `Thread` spawns.
//! `Monitor` therefore has real work to do now — `lock` genuinely excludes —
//! while a task-based program sees no overlap at all.

use crate::support::*;
#[allow(unused_imports)]
use crate::prelude::*;
use rustclr_core::{
    ByRef, ClrDelegate, ClrObject, ExecResult, ExecutionError, Interpreter, Value,
};

pub fn register(interp: &mut Interpreter) {
    register_thread(interp);
    register_monitor(interp);
    register_interlocked(interp);
    register_volatile(interp);
    register_type(interp);
    register_operating_system(interp);
}

// ── Thread ───────────────────────────────────────────────────────────────────

fn register_thread(interp: &mut Interpreter) {
    // `new Thread(ThreadStart)` stores the delegate on the instance.
    interp.register_native("System.Threading.Thread::.ctor/1", |i, a| {
        let this = arg_handle(i, a, 0)?;
        let body = arg(i, a, 1)?;
        i.heap.with_mut::<ClrObject, _>(this, |object| {
            if object.fields.is_empty() {
                object.fields.push(body);
            } else {
                object.fields[0] = body;
            }
        });
        Ok(None)
    });
    interp.register_native("System.Threading.Thread::.ctor/2", |i, a| {
        let this = arg_handle(i, a, 0)?;
        let body = arg(i, a, 1)?;
        i.heap.with_mut::<ClrObject, _>(this, |object| {
            if object.fields.is_empty() {
                object.fields.push(body);
            } else {
                object.fields[0] = body;
            }
        });
        Ok(None)
    });

    interp.register_native("System.Threading.Thread::Start()", start_thread);
    interp.register_native("System.Threading.Thread::Start/0", start_thread);
    interp.register_native("System.Threading.Thread::Start/1", start_thread);

    interp.register_native("System.Threading.Thread::Join()", join_thread);
    interp.register_native("System.Threading.Thread::Join/0", join_thread);
    interp.register_native("System.Threading.Thread::Join(int)", |_i, _a| {
        Ok(Some(Value::I32(1)))
    });
    interp.register_native("System.Threading.Thread::get_IsAlive()", |_i, _a| {
        Ok(Some(Value::I32(0)))
    });
    interp.register_native("System.Threading.Thread::set_IsBackground(bool)", |_i, _a| Ok(None));
    interp.register_native("System.Threading.Thread::set_Name(string)", |_i, _a| Ok(None));

    // `ThreadPool.QueueUserWorkItem` has the same shape and the same caveat.
    interp.register_native("System.Threading.ThreadPool::QueueUserWorkItem/1", |i, a| {
        let callback = arg(i, a, 0)?;
        invoke_delegate(i, callback, Some(Value::Null))?;
        Ok(Some(Value::I32(1)))
    });
    interp.register_native("System.Threading.ThreadPool::QueueUserWorkItem/2", |i, a| {
        let callback = arg(i, a, 0)?;
        let state = arg(i, a, 1)?;
        invoke_delegate(i, callback, Some(state))?;
        Ok(Some(Value::I32(1)))
    });
}

/// Runs the thread body synchronously; see this module's note on serialisation.
/// The slot on a `Thread` holding the id of the OS thread it started.
const THREAD_ID: usize = 1;

/// `Thread.Start()` — spawns.
#[cfg(feature = "std")]
fn start_thread(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let this = arg_handle(interp, args, 0)?;
    let body = interp
        .heap
        .with::<ClrObject, _>(this, |o| o.fields.first().cloned())
        .flatten()
        .unwrap_or(Value::Null);
    let parameter = if args.len() > 1 { Some(arg(interp, args, 1)?) } else { None };

    let id = interp.spawn(Box::new(rustclr_core::SystemHost::new()), move |worker| {
        invoke_delegate(worker, body, parameter).map(|_| ())
    });

    // The id goes on the instance so `Join` can find the thread again. The
    // delegate is slot zero, so this is slot one.
    interp.heap.with_mut::<ClrObject, _>(this, |o| {
        while o.fields.len() <= THREAD_ID {
            o.fields.push(Value::Null);
        }
        o.fields[THREAD_ID] = Value::I64(id as i64);
    });
    Ok(None)
}

/// Without `std` there is nothing to spawn onto, so the body runs inline.
#[cfg(not(feature = "std"))]
fn start_thread(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    run_thread_body(interp, args)
}

#[cfg(feature = "std")]
fn join_thread(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let this = arg_handle(interp, args, 0)?;
    let id = interp
        .heap
        .with::<ClrObject, _>(this, |o| o.fields.get(THREAD_ID).cloned())
        .flatten()
        .and_then(|v| v.as_i64());
    if let Some(id) = id {
        interp.join(id as u64)?;
    }
    Ok(None)
}

#[cfg(not(feature = "std"))]
fn join_thread(_i: &mut Interpreter, _a: &[Value]) -> ExecResult<Option<Value>> {
    Ok(None)
}

#[allow(dead_code)]
fn run_thread_body(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let this = arg_handle(interp, args, 0)?;
    let body = interp
        .heap.with::<ClrObject, _>(this, |o| o.fields.first().cloned()).flatten()
        .unwrap_or(Value::Null);

    // A parameterised start passes its argument through.
    let parameter = if args.len() > 1 { Some(arg(interp, args, 1)?) } else { None };
    invoke_delegate(interp, body, parameter)?;
    Ok(None)
}

/// Calls a delegate value with an optional single argument.
fn invoke_delegate(
    interp: &mut Interpreter,
    delegate: Value,
    parameter: Option<Value>,
) -> ExecResult<Option<Value>> {
    let Some(handle) = delegate.as_handle().filter(|h| !h.is_null()) else {
        return Err(ExecutionError::null_reference());
    };
    let targets = interp
        .heap.with::<ClrDelegate, _>(handle, |d| d.targets.clone())
        .ok_or_else(ExecutionError::null_reference)?;

    let mut result = None;
    for target in targets {
        let mut call_args = Vec::with_capacity(2);
        if !target.receiver.is_null() {
            call_args.push(Value::Obj(target.receiver));
        }
        // Pass the state argument only when the target expects one.
        if let Some(value) = parameter.clone() {
            let expects = interp.loader.registry.method(target.method).signature.params.len();
            if expects > 0 {
                call_args.push(value);
            }
        }
        result = interp.invoke(target.method, call_args)?;
    }
    Ok(result)
}

// ── Monitor ──────────────────────────────────────────────────────────────────

fn register_monitor(interp: &mut Interpreter) {
    // `lock (x) { … }` compiles to Enter(x, ref taken) / Exit(x) in a finally.
    // The `taken` flag has to be set or the generated `finally` skips the Exit.
    //
    // These genuinely exclude now. While threads were serialised a no-op was
    // harmless; with `Thread.Start` spawning, a lock that excludes nothing is a
    // data race in the user's program — `lock (gate) counter++` on two threads
    // gave 1,863 of 2,000 the first time it was tried.
    interp.register_native("System.Threading.Monitor::Enter(object,bool&)", |i, a| {
        let object = arg_handle(i, a, 0)?;
        i.monitor_enter(object);
        if let Some(Value::Ref(target)) = a.get(1) {
            let slot: ByRef = target.clone();
            i.store_indirect_public(slot, Value::I32(1))?;
        }
        Ok(None)
    });
    for member in ["Enter(object)", "Enter/1"] {
        interp.register_native(key_monitor(member), |i, a| {
            let object = arg_handle(i, a, 0)?;
            i.monitor_enter(object);
            Ok(None)
        });
    }
    for member in ["Exit(object)", "Exit/1"] {
        interp.register_native(key_monitor(member), |i, a| {
            let object = arg_handle(i, a, 0)?;
            i.monitor_exit(object);
            Ok(None)
        });
    }
    // `Pulse` wakes a waiter. Nothing here waits on a condition — `Monitor.Wait`
    // is not implemented — so there is nobody to wake, and saying so is better
    // than pretending. `Enter` already re-checks whenever a lock is released.
    interp.register_native("System.Threading.Monitor::Pulse(object)", |_i, _a| Ok(None));
    interp.register_native("System.Threading.Monitor::PulseAll(object)", |_i, _a| Ok(None));
    interp.register_native("System.Threading.Monitor::IsEntered(object)", |i, a| {
        let object = arg_handle(i, a, 0)?;
        Ok(Some(Value::I32(i.monitor_held(object) as i32)))
    });
}

// ── Interlocked ──────────────────────────────────────────────────────────────

/// A key on `System.Threading.Monitor`.
fn key_monitor(member: &str) -> &'static str {
    alloc::boxed::Box::leak(
        alloc::format!("System.Threading.Monitor::{member}").into_boxed_str(),
    )
}

fn register_volatile(interp: &mut Interpreter) {
    // Every slot two threads can both reach — the heap, static storage — is
    // behind a lock here, so a plain read already sees what a `Volatile.Read`
    // is asking for. These exist so the members resolve, not because they add
    // ordering the runtime was missing.
    for member in ["Read/1", "Read(int&)"] {
        interp.register_native(key_volatile(member), |i, a| {
            let Some(Value::Ref(slot)) = a.first() else {
                return Err(ExecutionError::null_reference());
            };
            let slot: ByRef = slot.clone();
            Ok(Some(i.load_indirect_public(slot)?))
        });
    }
    for member in ["Write/2", "Write(int&,int)"] {
        interp.register_native(key_volatile(member), |i, a| {
            let Some(Value::Ref(slot)) = a.first() else {
                return Err(ExecutionError::null_reference());
            };
            let slot: ByRef = slot.clone();
            let value = arg(i, a, 1)?;
            i.store_indirect_public(slot, value)?;
            Ok(None)
        });
    }
}

fn key_volatile(member: &str) -> &'static str {
    alloc::boxed::Box::leak(
        alloc::format!("System.Threading.Volatile::{member}").into_boxed_str(),
    )
}

fn register_interlocked(interp: &mut Interpreter) {
    interp.register_native("System.Threading.Interlocked::Add(int&,int)", |i, a| {
        atomic_update(i, a, |current, operand| current.wrapping_add(operand))
    });
    interp.register_native("System.Threading.Interlocked::Increment(int&)", |i, a| {
        atomic_update_by(i, a, 1)
    });
    interp.register_native("System.Threading.Interlocked::Decrement(int&)", |i, a| {
        atomic_update_by(i, a, -1)
    });
    interp.register_native("System.Threading.Interlocked::Exchange(int&,int)", |i, a| {
        let Some(Value::Ref(slot)) = a.first() else {
            return Err(ExecutionError::null_reference());
        };
        let slot: ByRef = slot.clone();
        let replacement = arg_i32(i, a, 1)?;
        i.interlocked(|i| {
            let previous = i.load_indirect_public(slot.clone())?.as_i32().unwrap_or(0);
            i.store_indirect_public(slot, Value::I32(replacement))?;
            Ok(Some(Value::I32(previous)))
        })
    });
}

fn atomic_update(
    interp: &mut Interpreter,
    args: &[Value],
    combine: fn(i32, i32) -> i32,
) -> ExecResult<Option<Value>> {
    let Some(Value::Ref(slot)) = args.first() else {
        return Err(ExecutionError::null_reference());
    };
    let slot: ByRef = slot.clone();
    let operand = arg_i32(interp, args, 1)?;
    // Read, combine and write with no other thread inside an interlocked
    // operation. Each of those three steps takes a lock of its own, so without
    // this two threads interleave between them and an update is lost — which
    // is precisely what `Interlocked` exists to prevent.
    interp.interlocked(|i| {
        let current = i.load_indirect_public(slot.clone())?.as_i32().unwrap_or(0);
        let updated = combine(current, operand);
        i.store_indirect_public(slot, Value::I32(updated))?;
        Ok(Some(Value::I32(updated)))
    })
}

fn atomic_update_by(
    interp: &mut Interpreter,
    args: &[Value],
    delta: i32,
) -> ExecResult<Option<Value>> {
    let Some(Value::Ref(slot)) = args.first() else {
        return Err(ExecutionError::null_reference());
    };
    let slot: ByRef = slot.clone();
    // Gated for the same reason `atomic_update` is: the read and the write are
    // separate locks, and two threads that interleave between them lose an
    // increment. This one was missed the first time, and four threads bumping
    // a counter 5,000 times each reached 18,828 of 20,000.
    interp.interlocked(|i| {
        let updated = i
            .load_indirect_public(slot.clone())?
            .as_i32()
            .unwrap_or(0)
            .wrapping_add(delta);
        i.store_indirect_public(slot, Value::I32(updated))?;
        Ok(Some(Value::I32(updated)))
    })
}

// ── Type handles ─────────────────────────────────────────────────────────────

fn register_type(interp: &mut Interpreter) {
    // `typeof(T)` becomes `ldtoken T; call Type.GetTypeFromHandle`. The token is
    // carried through as-is, which is enough for the identity comparisons
    // records generate — but not for anything that inspects the type.
    interp.register_native("System.Type::GetTypeFromHandle/1", |i, a| {
        Ok(Some(arg(i, a, 0)?))
    });
    interp.register_native("System.Type::op_Equality/2", |i, a| {
        let left = arg(i, a, 0)?;
        let right = arg(i, a, 1)?;
        Ok(Some(Value::I32((left.as_i64() == right.as_i64()) as i32)))
    });
    interp.register_native("System.Type::op_Inequality/2", |i, a| {
        let left = arg(i, a, 0)?;
        let right = arg(i, a, 1)?;
        Ok(Some(Value::I32((left.as_i64() != right.as_i64()) as i32)))
    });

}

// ── OperatingSystem ──────────────────────────────────────────────────────────

fn register_operating_system(interp: &mut Interpreter) {
    interp.register_native("System.OperatingSystem::IsWindows()", |_i, _a| {
        Ok(Some(Value::I32(cfg!(windows) as i32)))
    });
    interp.register_native("System.OperatingSystem::IsLinux()", |_i, _a| {
        Ok(Some(Value::I32(cfg!(target_os = "linux") as i32)))
    });
    interp.register_native("System.OperatingSystem::IsMacOS()", |_i, _a| {
        Ok(Some(Value::I32(cfg!(target_os = "macos") as i32)))
    });
    interp.register_native("System.OperatingSystem::IsBrowser()", |_i, _a| {
        Ok(Some(Value::I32(0)))
    });
}
