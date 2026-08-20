//! `System.Threading.Tasks.Task` and the `async`/`await` machinery.
//!
//! An `async` method is not special to the runtime. Roslyn lowers it to an
//! ordinary struct — the *state machine* — plus calls into a *builder*:
//!
//! ```text
//! ldloca sm; call AsyncTaskMethodBuilder`1::Create(); stfld <>t__builder
//! ldloca sm; ldc.i4.m1; stfld <>1__state
//! ldloca sm; ldflda <>t__builder; ldloca sm; call Start<TStateMachine>(ref)
//! ldloca sm; ldflda <>t__builder; call get_Task(); ret
//! ```
//!
//! `Start` runs `MoveNext` immediately. `MoveNext` runs the method body until
//! it reaches an `await` whose task is not finished; there it saves its
//! position, hands the builder the awaiter and itself, and returns. When the
//! awaited task completes, the builder calls `MoveNext` again and the method
//! resumes where it left off.
//!
//! Implementing those calls is the whole of `await` support. The state machine
//! is ordinary IL that the interpreter already runs.
//!
//! # Suspension needs a heap copy
//!
//! In a release build the state machine is a **struct in the caller's local**.
//! The moment a method suspends, that local is gone — so `AwaitUnsafeOnCompleted`
//! copies the machine into a one-field heap cell and resumes it through a
//! managed pointer at that cell. This is the same device `newobj` uses to give
//! a value-type constructor something to write through, and it is why the
//! resumed machine sees its own saved fields rather than a stale copy.
//!
//! # Asynchrony is synchronous
//!
//! There is one interpreter thread, so a task runs to completion at the point
//! it is created: `Task.Run` invokes its delegate immediately, and `Task.Delay`
//! sleeps. Results, ordering and exception propagation are correct, and a
//! program that awaits several tasks and joins their results produces exactly
//! what .NET produces. What is absent is *overlap* — two tasks never make
//! progress at the same time, so code that depends on interleaving will not
//! interleave. `rustnet capabilities` says so, and so does
//! `docs/limitations.md`.
//!
//! The continuation path is real regardless: a `TaskCompletionSource` completed
//! later genuinely suspends its awaiter and resumes it on completion.

use crate::collections::{elements, field, field_handle, new_list, sequence_values, set_field};
use crate::support::*;
use rustclr_core::{
    ClrDelegate, ClrExceptionKind, ClrObject, ExecResult, ExecutionError, Interpreter, MethodId,
    MethodKind, TypeId, Value,
};
use rustclr_gc::Handle;

const TASKS: &str = "System.Threading.Tasks";
const CS: &str = "System.Runtime.CompilerServices";

/// Task field slots.
const STATUS: usize = 0;
const RESULT: usize = 1;
const EXCEPTION: usize = 2;
const CONTINUATIONS: usize = 3;

const PENDING: i32 = 0;
const COMPLETED: i32 = 1;
const FAULTED: i32 = 2;

/// Leaks a stable key string; the native table holds it for the process life.
fn key(type_name: &str, member: &str) -> &'static str {
    Box::leak(format!("{type_name}::{member}").into_boxed_str())
}

pub fn register(interp: &mut Interpreter) {
    register_task(interp);
    register_builders(interp);
    register_awaiters(interp);
    register_completion_source(interp);
}

// -- the task object ---------------------------------------------------------

fn task_type(interp: &Interpreter, generic: bool) -> Option<TypeId> {
    let name = if generic { "Task`1" } else { "Task" };
    interp.loader.registry.find_type_by_name(&format!("{TASKS}.{name}"))
}

/// Allocates a task in the pending state.
fn new_task(interp: &mut Interpreter, generic: bool) -> Handle {
    let Some(type_id) = task_type(interp, generic) else { return Handle::NULL };
    let handle = interp.alloc_object(type_id);
    set_field(interp, handle, STATUS, Value::I32(PENDING));
    set_field(interp, handle, RESULT, Value::Null);
    set_field(interp, handle, EXCEPTION, Value::Null);
    let list = interp.alloc_value_array(0);
    set_field(interp, handle, CONTINUATIONS, Value::Obj(list));
    handle
}

/// A task already carrying its result.
pub(crate) fn completed_task(interp: &mut Interpreter, result: Value) -> Handle {
    let generic = !matches!(result, Value::Null);
    let handle = new_task(interp, generic);
    set_field(interp, handle, STATUS, Value::I32(COMPLETED));
    set_field(interp, handle, RESULT, result);
    handle
}

fn status_of(interp: &Interpreter, task: Handle) -> i32 {
    field(interp, task, STATUS).as_i32().unwrap_or(PENDING)
}

/// Marks a task finished and resumes everything waiting on it.
fn complete(interp: &mut Interpreter, task: Handle, result: Value) -> ExecResult<()> {
    if status_of(interp, task) != PENDING {
        return Err(ExecutionError::exception(
            ClrExceptionKind::InvalidOperation,
            "An attempt was made to transition a task to a final state when it had already completed.",
        ));
    }
    set_field(interp, task, STATUS, Value::I32(COMPLETED));
    set_field(interp, task, RESULT, result);
    run_continuations(interp, task)
}

fn fault(interp: &mut Interpreter, task: Handle, exception: Value) -> ExecResult<()> {
    set_field(interp, task, STATUS, Value::I32(FAULTED));
    set_field(interp, task, EXCEPTION, exception);
    run_continuations(interp, task)
}

/// Resumes every state machine registered on a completed task.
///
/// The list is taken and cleared first: a resumed machine may register another
/// continuation on this same task, and appending to a list being iterated would
/// either loop forever or miss the new entry.
fn run_continuations(interp: &mut Interpreter, task: Handle) -> ExecResult<()> {
    let list = field_handle(interp, task, CONTINUATIONS);
    if list.is_null() {
        return Ok(());
    }
    let waiting = elements(interp, list);
    if waiting.is_empty() {
        return Ok(());
    }
    let fresh = interp.alloc_value_array(0);
    set_field(interp, task, CONTINUATIONS, Value::Obj(fresh));

    for cell in waiting {
        if let Some(handle) = cell.as_handle().filter(|h| !h.is_null()) {
            resume(interp, handle)?;
        }
    }
    Ok(())
}

/// Calls `MoveNext` on a state machine held in a one-field heap cell.
fn resume(interp: &mut Interpreter, cell: Handle) -> ExecResult<()> {
    let machine = interp
        .heap
        .get_as::<ClrObject>(cell)
        .and_then(|o| o.fields.first().cloned())
        .unwrap_or(Value::Null);
    let Some(type_id) = state_machine_type(interp, &machine) else {
        return Err(ExecutionError::InvalidProgram(
            "a suspended async method lost its state machine".into(),
        ));
    };
    let Some(move_next) = find_move_next(interp, type_id) else {
        return Err(ExecutionError::MissingImplementation(format!(
            "{} has no MoveNext, so the async method cannot resume",
            interp.loader.registry.ty(type_id).full_name()
        )));
    };
    // `this` is a pointer at the cell's slot, so the machine's own `stfld`
    // writes land in the copy that will be resumed next time.
    let this = Value::Ref(rustclr_core::ByRef::Field { object: cell, slot: 0 });
    interp.invoke(move_next, vec![this])?;
    Ok(())
}

fn state_machine_type(interp: &Interpreter, machine: &Value) -> Option<TypeId> {
    match machine {
        Value::Struct(s) => Some(s.type_id),
        Value::Obj(h) => interp.type_of(*h),
        _ => None,
    }
}

/// A state machine implements `IAsyncStateMachine` explicitly, so `MoveNext`
/// may be emitted under a qualified name.
fn find_move_next(interp: &Interpreter, type_id: TypeId) -> Option<MethodId> {
    for t in interp.loader.registry.base_chain(type_id) {
        for m in &interp.loader.registry.ty(t).methods {
            let info = interp.loader.registry.method(*m);
            if info.signature.params.is_empty()
                && matches!(info.kind, MethodKind::Il(_))
                && (info.name == "MoveNext" || info.name.ends_with(".MoveNext"))
            {
                return Some(*m);
            }
        }
    }
    None
}

/// The task a value refers to, whatever wrapper it arrived in.
fn as_task(interp: &mut Interpreter, args: &[Value], index: usize) -> ExecResult<Handle> {
    let v = arg(interp, args, index)?;
    v.as_handle().filter(|h| !h.is_null()).ok_or_else(ExecutionError::null_reference)
}

/// Reads a finished task's value, rethrowing if it faulted.
fn result_of(interp: &mut Interpreter, task: Handle) -> ExecResult<Value> {
    match status_of(interp, task) {
        FAULTED => {
            let exception = field(interp, task, EXCEPTION);
            match exception.as_handle().filter(|h| !h.is_null()) {
                // Rethrow the original instance, so `catch` sees the exception
                // the failing task actually threw rather than a stand-in.
                Some(object) => {
                    let message = interp
                        .heap
                        .get_as::<rustclr_core::ClrException>(object)
                        .map(|e| e.message.clone())
                        .unwrap_or_default();
                    Err(ExecutionError::Exception {
                        kind: ClrExceptionKind::InvalidOperation,
                        message,
                        object,
                    })
                }
                None => Err(ExecutionError::exception(
                    ClrExceptionKind::InvalidOperation,
                    "A task failed without recording an exception.",
                )),
            }
        }
        COMPLETED => Ok(field(interp, task, RESULT)),
        // Nothing else can run to complete it: see the module note on
        // asynchrony being synchronous.
        _ => Err(ExecutionError::exception(
            ClrExceptionKind::InvalidOperation,
            "This task never completed. Asynchrony is synchronous on this runtime, so a task \
             awaited before anything completes it cannot make progress. See docs/limitations.md.",
        )),
    }
}

fn register_task(interp: &mut Interpreter) {
    for name in [format!("{TASKS}.Task"), format!("{TASKS}.Task`1")] {
        let name: &'static str = Box::leak(name.into_boxed_str());

        // The awaiter *is* the task: both are a single reference, and every
        // awaiter member here reads through it.
        interp.register_native(key(name, "GetAwaiter()"), |i, a| {
            Ok(Some(Value::Obj(as_task(i, a, 0)?)))
        });
        interp.register_native(key(name, "ConfigureAwait(bool)"), |i, a| {
            Ok(Some(Value::Obj(as_task(i, a, 0)?)))
        });
        interp.register_native(key(name, "ConfigureAwait/1"), |i, a| {
            Ok(Some(Value::Obj(as_task(i, a, 0)?)))
        });

        interp.register_native(key(name, "get_Result()"), |i, a| {
            let task = as_task(i, a, 0)?;
            Ok(Some(result_of(i, task)?))
        });
        interp.register_native(key(name, "get_IsCompleted()"), |i, a| {
            let task = as_task(i, a, 0)?;
            Ok(Some(Value::I32((status_of(i, task) != PENDING) as i32)))
        });
        interp.register_native(key(name, "get_IsCompletedSuccessfully()"), |i, a| {
            let task = as_task(i, a, 0)?;
            Ok(Some(Value::I32((status_of(i, task) == COMPLETED) as i32)))
        });
        interp.register_native(key(name, "get_IsFaulted()"), |i, a| {
            let task = as_task(i, a, 0)?;
            Ok(Some(Value::I32((status_of(i, task) == FAULTED) as i32)))
        });
        interp.register_native(key(name, "get_IsCanceled()"), |_i, _a| Ok(Some(Value::I32(0))));
        interp.register_native(key(name, "get_Exception()"), |i, a| {
            let task = as_task(i, a, 0)?;
            Ok(Some(field(i, task, EXCEPTION)))
        });
        interp.register_native(key(name, "Wait()"), |i, a| {
            let task = as_task(i, a, 0)?;
            result_of(i, task)?;
            Ok(None)
        });
        interp.register_native(key(name, "Wait(int)"), |i, a| {
            let task = as_task(i, a, 0)?;
            result_of(i, task)?;
            Ok(Some(Value::I32(1)))
        });
    }

    // Statics. `Task.Run` invokes its delegate at once — see the module note.
    interp.register_native(key(&format!("{TASKS}.Task"), "get_CompletedTask()"), |i, _a| {
        let task = new_task(i, false);
        set_field(i, task, STATUS, Value::I32(COMPLETED));
        Ok(Some(Value::Obj(task)))
    });
    interp.register_native(key(&format!("{TASKS}.Task"), "FromResult/1"), |i, a| {
        let value = arg(i, a, 0)?;
        Ok(Some(Value::Obj(completed_task(i, value))))
    });
    interp.register_native(key(&format!("{TASKS}.Task"), "Delay(int)"), |i, a| {
        let ms = arg_i32(i, a, 0)?.max(0) as u64;
        std::thread::sleep(std::time::Duration::from_millis(ms));
        let task = new_task(i, false);
        set_field(i, task, STATUS, Value::I32(COMPLETED));
        Ok(Some(Value::Obj(task)))
    });
    interp.register_native(key(&format!("{TASKS}.Task"), "Yield()"), |i, _a| {
        let task = new_task(i, false);
        set_field(i, task, STATUS, Value::I32(COMPLETED));
        Ok(Some(Value::Obj(task)))
    });
    interp.register_native(key(&format!("{TASKS}.Task"), "Run/1"), run_delegate);
    interp.register_native(key(&format!("{TASKS}.Task"), "Run/2"), run_delegate);

    interp.register_native(key(&format!("{TASKS}.Task"), "WhenAll/1"), |i, a| {
        let source = arg(i, a, 0)?;
        let tasks = sequence_values(i, &source);
        let mut results = Vec::with_capacity(tasks.len());
        for t in tasks {
            if let Some(h) = t.as_handle().filter(|h| !h.is_null()) {
                results.push(result_of(i, h)?);
            }
        }
        // `WhenAll` over `Task<T>` yields the results; over `Task` it yields
        // nothing. Carrying the list is correct for both — an awaiter that
        // wants no value simply ignores it.
        let list = new_list(i, results);
        Ok(Some(Value::Obj(completed_task(i, list))))
    });
    interp.register_native(key(&format!("{TASKS}.Task"), "WhenAny/1"), |i, a| {
        let source = arg(i, a, 0)?;
        let tasks = sequence_values(i, &source);
        // Every task is already finished here, so the first one is the answer,
        // exactly as .NET would report if all had completed before the call.
        let first = tasks.into_iter().next().unwrap_or(Value::Null);
        Ok(Some(Value::Obj(completed_task(i, first))))
    });
}

/// `Task.Run(Func<T>)` / `Task.Run(Action)`.
fn run_delegate(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let body = args.first().cloned().unwrap_or(Value::Null);
    match invoke_delegate(interp, &body, &[]) {
        Ok(value) => Ok(Some(Value::Obj(completed_task(interp, value)))),
        Err(e) => {
            // A failure inside the delegate becomes a faulted task rather than
            // escaping here, which is what `await` expects to observe.
            let task = new_task(interp, true);
            let exception = exception_value(&e);
            fault(interp, task, exception)?;
            Ok(Some(Value::Obj(task)))
        }
    }
}

/// The managed exception object an error carries, if it has one.
fn exception_value(error: &ExecutionError) -> Value {
    match error {
        ExecutionError::Exception { object, .. } => Value::Obj(*object),
        _ => Value::Null,
    }
}

fn invoke_delegate(
    interp: &mut Interpreter,
    delegate: &Value,
    args: &[Value],
) -> ExecResult<Value> {
    let handle = delegate
        .as_handle()
        .filter(|h| !h.is_null())
        .ok_or_else(ExecutionError::null_reference)?;
    let targets = interp
        .heap
        .get_as::<ClrDelegate>(handle)
        .map(|d| d.targets.clone())
        .ok_or_else(ExecutionError::null_reference)?;

    let mut result = Value::Null;
    for target in targets {
        let mut call_args = Vec::with_capacity(args.len() + 1);
        if !target.receiver.is_null() {
            call_args.push(Value::Obj(target.receiver));
        }
        call_args.extend_from_slice(args);
        result = interp.invoke(target.method, call_args)?.unwrap_or(Value::Null);
    }
    Ok(result)
}

// -- the builders ------------------------------------------------------------

fn register_builders(interp: &mut Interpreter) {
    for name in [
        "AsyncTaskMethodBuilder",
        "AsyncTaskMethodBuilder`1",
        "AsyncVoidMethodBuilder",
        "AsyncValueTaskMethodBuilder",
        "AsyncValueTaskMethodBuilder`1",
    ] {
        let full: &'static str = Box::leak(format!("{CS}.{name}").into_boxed_str());
        let generic = name.ends_with("`1");

        // `Create()` is static and returns the builder by value. The builder's
        // single slot holds the task it will complete.
        interp.register_native(key(full, "Create()"), move |i, _a| {
            let task = new_task(i, true);
            Ok(Some(Value::Obj(task)))
        });
        let _ = generic;

        interp.register_native(key(full, "get_Task()"), |i, a| Ok(Some(arg(i, a, 0)?)));

        interp.register_native(key(full, "SetResult()"), |i, a| {
            let task = as_task(i, a, 0)?;
            complete(i, task, Value::Null)?;
            Ok(None)
        });
        interp.register_native(key(full, "SetResult(!0)"), |i, a| {
            let task = as_task(i, a, 0)?;
            let value = arg(i, a, 1)?;
            complete(i, task, value)?;
            Ok(None)
        });
        interp.register_native(key(full, "SetResult/1"), |i, a| {
            let task = as_task(i, a, 0)?;
            let value = arg(i, a, 1)?;
            complete(i, task, value)?;
            Ok(None)
        });

        interp.register_native(key(full, "SetException/1"), |i, a| {
            let task = as_task(i, a, 0)?;
            let exception = arg(i, a, 1)?;
            fault(i, task, exception)?;
            Ok(None)
        });

        // The state machine is already reachable through the builder's task.
        interp.register_native(key(full, "SetStateMachine/1"), |_i, _a| Ok(None));

        interp.register_native(key(full, "Start/1"), start_state_machine);
        interp.register_native(key(full, "AwaitOnCompleted/2"), await_on_completed);
        interp.register_native(key(full, "AwaitUnsafeOnCompleted/2"), await_on_completed);
    }
}

/// `Start<TStateMachine>(ref TStateMachine)` — runs the body until it either
/// finishes or reaches an `await` that cannot complete immediately.
fn start_state_machine(
    interp: &mut Interpreter,
    args: &[Value],
) -> ExecResult<Option<Value>> {
    let machine_ref = args.get(1).cloned().unwrap_or(Value::Null);
    let machine = match &machine_ref {
        Value::Ref(r) => interp.load_indirect_public(r.clone())?,
        other => other.clone(),
    };
    let Some(type_id) = state_machine_type(interp, &machine) else {
        return Err(ExecutionError::InvalidProgram(
            "an async method builder was started without a state machine".into(),
        ));
    };
    let Some(move_next) = find_move_next(interp, type_id) else {
        return Err(ExecutionError::MissingImplementation(format!(
            "{} has no MoveNext",
            interp.loader.registry.ty(type_id).full_name()
        )));
    };
    // Pass the pointer straight through, so the body's writes to its own
    // fields land in the caller's local.
    interp.invoke(move_next, vec![machine_ref])?;
    Ok(None)
}

/// `AwaitUnsafeOnCompleted<TAwaiter, TStateMachine>(ref awaiter, ref machine)`.
///
/// Reached only when the awaited task is still pending. The machine is copied
/// to the heap — its local is about to disappear — and registered on the task.
fn await_on_completed(
    interp: &mut Interpreter,
    args: &[Value],
) -> ExecResult<Option<Value>> {
    let awaited = arg(interp, args, 1)?;
    let machine = arg(interp, args, 2)?;

    let Some(task) = awaited.as_handle().filter(|h| !h.is_null()) else {
        return Err(ExecutionError::null_reference());
    };

    let object_type = interp.loader.core().object;
    let mut cell = ClrObject::new(object_type, 1);
    cell.fields[0] = machine;
    let cell = interp.heap.alloc(cell);

    // Already finished — resume at once rather than queueing something nothing
    // will ever drain.
    if status_of(interp, task) != PENDING {
        return resume(interp, cell).map(|_| None);
    }

    let list = field_handle(interp, task, CONTINUATIONS);
    if list.is_null() {
        let fresh = interp.alloc_value_array(0);
        set_field(interp, task, CONTINUATIONS, Value::Obj(fresh));
    }
    let list = field_handle(interp, task, CONTINUATIONS);
    crate::collections::push_value(interp, list, Value::Obj(cell));
    Ok(None)
}

// -- awaiters ----------------------------------------------------------------

fn register_awaiters(interp: &mut Interpreter) {
    for name in [
        "TaskAwaiter",
        "TaskAwaiter`1",
        "ConfiguredTaskAwaitable+ConfiguredTaskAwaiter",
        "ConfiguredTaskAwaitable`1+ConfiguredTaskAwaiter",
        "YieldAwaitable+YieldAwaiter",
    ] {
        let full: &'static str = Box::leak(format!("{CS}.{name}").into_boxed_str());

        interp.register_native(key(full, "get_IsCompleted()"), |i, a| {
            let task = as_task(i, a, 0)?;
            Ok(Some(Value::I32((status_of(i, task) != PENDING) as i32)))
        });
        interp.register_native(key(full, "GetResult()"), |i, a| {
            let task = as_task(i, a, 0)?;
            let value = result_of(i, task)?;
            Ok(Some(value))
        });
        interp.register_native(key(full, "OnCompleted/1"), |_i, _a| Ok(None));
        interp.register_native(key(full, "UnsafeOnCompleted/1"), |_i, _a| Ok(None));
    }

    // `ConfigureAwait` and `Task.Yield` hand back the same reference, so their
    // awaitables need only produce themselves.
    for name in [
        "ConfiguredTaskAwaitable",
        "ConfiguredTaskAwaitable`1",
        "YieldAwaitable",
    ] {
        let full: &'static str = Box::leak(format!("{CS}.{name}").into_boxed_str());
        interp.register_native(key(full, "GetAwaiter()"), |i, a| Ok(Some(arg(i, a, 0)?)));
    }
}

// -- TaskCompletionSource ----------------------------------------------------

fn register_completion_source(interp: &mut Interpreter) {
    for name in ["TaskCompletionSource", "TaskCompletionSource`1"] {
        let full: &'static str = Box::leak(format!("{TASKS}.{name}").into_boxed_str());

        for ctor in [".ctor()", ".ctor/0", ".ctor/1"] {
            interp.register_native(key(full, ctor), |i, a| {
                let this = arg_handle(i, a, 0)?;
                let task = new_task(i, true);
                set_field(i, this, 0, Value::Obj(task));
                Ok(None)
            });
        }
        interp.register_native(key(full, "get_Task()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            Ok(Some(field(i, this, 0)))
        });
        interp.register_native(key(full, "SetResult/1"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let task = field_handle(i, this, 0);
            let value = arg(i, a, 1)?;
            complete(i, task, value)?;
            Ok(None)
        });
        interp.register_native(key(full, "SetResult()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let task = field_handle(i, this, 0);
            complete(i, task, Value::Null)?;
            Ok(None)
        });
        interp.register_native(key(full, "TrySetResult/1"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let task = field_handle(i, this, 0);
            if status_of(i, task) != PENDING {
                return Ok(Some(Value::I32(0)));
            }
            let value = arg(i, a, 1)?;
            complete(i, task, value)?;
            Ok(Some(Value::I32(1)))
        });
        interp.register_native(key(full, "SetException/1"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let task = field_handle(i, this, 0);
            let exception = arg(i, a, 1)?;
            fault(i, task, exception)?;
            Ok(None)
        });
    }
}
