//! `System.Threading` and a few reflection-adjacent members.
//!
//! # Threads are serialised
//!
//! RustCLR has one interpreter, and it is not re-entrant across OS threads.
//! `Thread.Start` therefore runs the delegate **synchronously on the calling
//! thread**, and `Join` returns immediately because the work is already done.
//!
//! That is correct for the common start-then-join shape and for anything that
//! uses threads to organise work rather than to gain parallelism. It is *wrong*
//! for a program that depends on two threads making progress at the same time —
//! a consumer that blocks waiting for a producer started later will hang.
//!
//! The alternative was to refuse `Thread` entirely. Serialising it makes more
//! programs run, so it is offered with the limitation stated here, in
//! `rustnet capabilities`, and in the documentation, rather than left for
//! someone to discover.
//!
//! `Monitor` follows from that: with no concurrent execution there is nothing to
//! exclude, so `lock` is a no-op that keeps its own recursion count.

use crate::support::*;
use rustclr_core::{
    ByRef, ClrDelegate, ClrObject, ExecResult, ExecutionError, Interpreter, Value,
};

pub fn register(interp: &mut Interpreter) {
    register_thread(interp);
    register_monitor(interp);
    register_interlocked(interp);
    register_type(interp);
    register_operating_system(interp);
}

// ── Thread ───────────────────────────────────────────────────────────────────

fn register_thread(interp: &mut Interpreter) {
    // `new Thread(ThreadStart)` stores the delegate on the instance.
    interp.register_native("System.Threading.Thread::.ctor/1", |i, a| {
        let this = arg_handle(i, a, 0)?;
        let body = arg(i, a, 1)?;
        if let Some(object) = i.heap.get_as_mut::<ClrObject>(this) {
            if object.fields.is_empty() {
                object.fields.push(body);
            } else {
                object.fields[0] = body;
            }
        }
        Ok(None)
    });
    interp.register_native("System.Threading.Thread::.ctor/2", |i, a| {
        let this = arg_handle(i, a, 0)?;
        let body = arg(i, a, 1)?;
        if let Some(object) = i.heap.get_as_mut::<ClrObject>(this) {
            if object.fields.is_empty() {
                object.fields.push(body);
            } else {
                object.fields[0] = body;
            }
        }
        Ok(None)
    });

    interp.register_native("System.Threading.Thread::Start()", run_thread_body);
    interp.register_native("System.Threading.Thread::Start/0", run_thread_body);
    interp.register_native("System.Threading.Thread::Start/1", run_thread_body);

    // The body already ran on Start, so there is nothing to wait for.
    interp.register_native("System.Threading.Thread::Join()", |_i, _a| Ok(None));
    interp.register_native("System.Threading.Thread::Join/0", |_i, _a| Ok(None));
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
fn run_thread_body(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let this = arg_handle(interp, args, 0)?;
    let body = interp
        .heap
        .get_as::<ClrObject>(this)
        .and_then(|o| o.fields.first().cloned())
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
        .heap
        .get_as::<ClrDelegate>(handle)
        .map(|d| d.targets.clone())
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
    // `lock (x) { … }` compiles to Enter(x, ref taken) / Exit(x). With threads
    // serialised there is nothing to exclude, but the `taken` flag must still be
    // set or the generated `finally` will skip the Exit.
    interp.register_native("System.Threading.Monitor::Enter(object,bool&)", |i, a| {
        if let Some(Value::Ref(target)) = a.get(1) {
            let slot: ByRef = *target;
            i.store_indirect_public(slot, Value::I32(1))?;
        }
        Ok(None)
    });
    interp.register_native("System.Threading.Monitor::Enter(object)", |_i, _a| Ok(None));
    interp.register_native("System.Threading.Monitor::Enter/1", |_i, _a| Ok(None));
    interp.register_native("System.Threading.Monitor::Exit(object)", |_i, _a| Ok(None));
    interp.register_native("System.Threading.Monitor::Exit/1", |_i, _a| Ok(None));
    interp.register_native("System.Threading.Monitor::Pulse(object)", |_i, _a| Ok(None));
    interp.register_native("System.Threading.Monitor::PulseAll(object)", |_i, _a| Ok(None));
}

// ── Interlocked ──────────────────────────────────────────────────────────────

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
        let slot: ByRef = *slot;
        let previous = i.load_indirect_public(slot)?.as_i32().unwrap_or(0);
        let replacement = arg_i32(i, a, 1)?;
        i.store_indirect_public(slot, Value::I32(replacement))?;
        Ok(Some(Value::I32(previous)))
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
    let slot: ByRef = *slot;
    let current = interp.load_indirect_public(slot)?.as_i32().unwrap_or(0);
    let operand = arg_i32(interp, args, 1)?;
    let updated = combine(current, operand);
    interp.store_indirect_public(slot, Value::I32(updated))?;
    Ok(Some(Value::I32(updated)))
}

fn atomic_update_by(
    interp: &mut Interpreter,
    args: &[Value],
    delta: i32,
) -> ExecResult<Option<Value>> {
    let Some(Value::Ref(slot)) = args.first() else {
        return Err(ExecutionError::null_reference());
    };
    let slot: ByRef = *slot;
    let updated = interp
        .load_indirect_public(slot)?
        .as_i32()
        .unwrap_or(0)
        .wrapping_add(delta);
    interp.store_indirect_public(slot, Value::I32(updated))?;
    Ok(Some(Value::I32(updated)))
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
