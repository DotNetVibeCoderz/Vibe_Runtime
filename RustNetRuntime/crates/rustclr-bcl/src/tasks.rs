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

#[allow(unused_imports)]
use crate::prelude::*;

const TASKS: &str = "System.Threading.Tasks";
const CS: &str = "System.Runtime.CompilerServices";

/// Task field slots.
const STATUS: usize = 0;
const RESULT: usize = 1;
const EXCEPTION: usize = 2;
const CONTINUATIONS: usize = 3;
/// The OS thread running this task's body, when `Task.Run` started one.
const THREAD: usize = 4;

const PENDING: i32 = 0;
const COMPLETED: i32 = 1;
const FAULTED: i32 = 2;

/// Leaks a stable key string; the native table holds it for the process life.
fn key(type_name: &str, member: &str) -> &'static str {
    Box::leak(format!("{type_name}::{member}").into_boxed_str())
}

pub fn register(interp: &mut Interpreter) {
    register_task(interp);
    register_value_task(interp);
    register_builders(interp);
    register_awaiters(interp);
    // After `register_awaiters`: the `ValueTask` awaiters it registers refuse a
    // null receiver, and these replace them with the null-tolerant versions.
    register_async_iterators(interp);
    register_completion_source(interp);
    register_parallel(interp);
}

/// `Parallel` and `Task.WaitAll` — **sequential**, and the name is the only
/// thing about them that says otherwise.
///
/// Nothing in this runtime overlaps yet: a task runs to completion where it is
/// created and `Thread.Start` runs its body inline. `Parallel.For` is the same
/// bargain. Running the body in order, once per index, produces the answer a
/// correctly written parallel loop would produce — those are required to be
/// independent of ordering, and one that is not was already broken.
///
/// What it does *not* give is speed, and it does not pretend to: the limitation
/// is in `docs/limitations.md` and in `rustnet capabilities`, beside the same
/// statement about `Thread`. Implementing the shape without the parallelism is
/// what lets a program that uses `Parallel.For` run at all, which is the
/// difference between "slower here" and "does not run here".
/// How many threads to use for a parallel loop of `items` iterations.
///
/// One per core, capped by the work available: spawning four threads for two
/// iterations costs more than it saves, and spawning one is just the sequential
/// loop with extra steps.
#[cfg(feature = "std")]
fn degree(items: usize) -> usize {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    cores.min(items).max(1)
}

/// Runs `body` once per element of `work`, across threads.
///
/// The chunks are contiguous rather than interleaved, which is what .NET's
/// range partitioner does and keeps each thread walking memory in order.
///
/// **Ordering is not preserved, and that is the contract.** A parallel loop's
/// body has to be independent of the order its iterations run in; one that is
/// not was already wrong. What *is* preserved is that every iteration runs
/// exactly once before this returns.
#[cfg(feature = "std")]
fn run_in_parallel(
    interp: &mut Interpreter,
    body: &Value,
    work: Vec<Vec<Value>>,
) -> ExecResult<()> {
    if work.len() <= 1 {
        // Nothing to gain, and a thread to pay for.
        for args in work {
            crate::linq::call(interp, body, &args)?;
        }
        return Ok(());
    }

    let threads = degree(work.len());
    let per = work.len().div_ceil(threads);
    let mut chunks: Vec<Vec<Vec<Value>>> = Vec::new();
    let mut rest = work;
    while !rest.is_empty() {
        let take = per.min(rest.len());
        chunks.push(rest.drain(..take).collect());
        }

    // A chunk per worker, queued on the pool. The loop waits for all of them,
    // and helps run them while it waits — so the caller's own thread is one of
    // the ones doing the work rather than idling.
    let outcome = Arc::new(Mutex::new(None));
    let remaining = Arc::new(AtomicUsize::new(chunks.len()));
    for chunk in chunks {
        let body = body.clone();
        let outcome = Arc::clone(&outcome);
        let remaining = Arc::clone(&remaining);
        interp.queue_work(Box::new(move |worker| {
            for args in chunk {
                if let Err(e) = crate::linq::call(worker, &body, &args) {
                    // .NET wraps the first failure in an `AggregateException`;
                    // this reports the failure itself, which is what the
                    // sequential version did and what a `catch` around a
                    // parallel loop on this runtime has always seen.
                    let mut slot = outcome.lock().expect("parallel outcome poisoned");
                    if slot.is_none() {
                        *slot = Some(e);
                    }
                    break;
                }
            }
            remaining.fetch_sub(1, Ordering::AcqRel);
        }));
    }

    // Every chunk finishes before this returns, failure or not: a body still
    // running after `Parallel.For` came back would be writing into a scope its
    // caller has left.
    while remaining.load(Ordering::Acquire) > 0 {
        if interp.help_with_queued_work() {
            continue;
        }
        interp.blocking(|_| std::thread::yield_now());
    }

    let error = outcome.lock().expect("parallel outcome poisoned").take();
    match error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// `Parallel.Invoke`: one thread per action.
#[cfg(feature = "std")]
fn run_each_in_parallel(interp: &mut Interpreter, actions: Vec<Value>) -> ExecResult<()> {
    if actions.len() <= 1 {
        for action in actions {
            crate::linq::call(interp, &action, &[])?;
        }
        return Ok(());
    }
    let work: Vec<Vec<Value>> = actions.iter().map(|_| Vec::new()).collect();
    let _ = work;
    let outcome = Arc::new(Mutex::new(None));
    let remaining = Arc::new(AtomicUsize::new(actions.len()));
    for action in actions {
        let outcome = Arc::clone(&outcome);
        let remaining = Arc::clone(&remaining);
        interp.queue_work(Box::new(move |worker| {
            if let Err(e) = crate::linq::call(worker, &action, &[]) {
                let mut slot = outcome.lock().expect("parallel outcome poisoned");
                if slot.is_none() {
                    *slot = Some(e);
                }
            }
            remaining.fetch_sub(1, Ordering::AcqRel);
        }));
    }
    while remaining.load(Ordering::Acquire) > 0 {
        if interp.help_with_queued_work() {
            continue;
        }
        interp.blocking(|_| std::thread::yield_now());
    }
    let error = outcome.lock().expect("parallel outcome poisoned").take();
    match error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Without `std` there is one thread, so a parallel loop is a loop.
#[cfg(not(feature = "std"))]
fn run_in_parallel(
    interp: &mut Interpreter,
    body: &Value,
    work: Vec<Vec<Value>>,
) -> ExecResult<()> {
    for args in work {
        crate::linq::call(interp, body, &args)?;
    }
    Ok(())
}

#[cfg(not(feature = "std"))]
fn run_each_in_parallel(interp: &mut Interpreter, actions: Vec<Value>) -> ExecResult<()> {
    for action in actions {
        crate::linq::call(interp, &action, &[])?;
    }
    Ok(())
}

/// The tasks a `WaitAll` / `WhenAll` argument holds.
///
/// It can be an array, or a `ReadOnlySpan<Task>` — .NET 10 lowers
/// `Task.WaitAll(a, b)` through an inline array and a span, so reading only
/// arrays here made `WaitAll` silently wait for nothing.
fn awaited_tasks(interp: &mut Interpreter, source: &Value) -> Vec<Value> {
    if let Some(values) = crate::spans::values(interp, source) {
        return values;
    }
    crate::collections::sequence_values(interp, source)
}

fn register_parallel(interp: &mut Interpreter) {
    let parallel: &'static str = Box::leak(format!("{TASKS}.Parallel").into_boxed_str());

    // `Parallel.For(from, to, body)`, and the overload taking loop state.
    for member in ["For/3", "For(int,int,System.Action`1)"] {
        interp.register_native(key(parallel, member), |i, a| {
            let from = arg_i32(i, a, 0)?;
            let to = arg_i32(i, a, 1)?;
            let body = arg(i, a, 2)?;
            let work: Vec<Vec<Value>> = (from..to).map(|n| alloc::vec![Value::I32(n)]).collect();
            run_in_parallel(i, &body, work)?;
            Ok(Some(Value::Null))
        });
    }

    // `Parallel.ForEach(source, body)`.
    interp.register_native(key(parallel, "ForEach/2"), |i, a| {
        let source = arg(i, a, 0)?;
        let items = crate::collections::sequence_values(i, &source);
        let body = arg(i, a, 1)?;
        let work: Vec<Vec<Value>> = items.into_iter().map(|v| alloc::vec![v]).collect();
        run_in_parallel(i, &body, work)?;
        Ok(Some(Value::Null))
    });

    // `Parallel.Invoke(params Action[])` — a different delegate per item, so
    // each chunk carries its own rather than sharing one.
    interp.register_native(key(parallel, "Invoke/1"), |i, a| {
        let source = arg(i, a, 0)?;
        let actions = crate::collections::sequence_values(i, &source);
        run_each_in_parallel(i, actions)?;
        Ok(None)
    });

    // `Task.WaitAll` / `Task.WaitAny`. These genuinely wait now: a task started
    // by `Task.Run` is running on another thread, and returning before it
    // finishes would let the caller read a result that does not exist yet.
    let task: &'static str = Box::leak(format!("{TASKS}.Task").into_boxed_str());
    for member in ["WaitAll/1", "WaitAll/2"] {
        interp.register_native(key(task, member), |i, a| {
            let source = arg(i, a, 0)?;
            for value in awaited_tasks(i, &source) {
                if let Some(h) = value.as_handle().filter(|h| !h.is_null()) {
                    settle(i, h)?;
                }
            }
            Ok(Some(Value::I32(0)))
        });
    }
    // `WaitAny` returns the index of one that finished. Waiting for the first
    // is what it means; waiting for *all* of them and reporting the first is
    // what this does, which returns the same index for a caller that then
    // inspects the tasks and takes longer for one that does not.
    interp.register_native(key(task, "WaitAny/1"), |i, a| {
        let source = arg(i, a, 0)?;
        for value in awaited_tasks(i, &source) {
            if let Some(h) = value.as_handle().filter(|h| !h.is_null()) {
                settle(i, h)?;
            }
        }
        Ok(Some(Value::I32(0)))
    });
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
    // Settling the status and taking the waiting list happen together, against
    // `await_on_completed` doing its check-and-add. Otherwise a task that
    // finishes between another thread's "is it pending?" and its "then wait for
    // it" loses that continuation, and the `await` never resumes.
    let taken = interp.interlocked(|i| {
        if status_of(i, task) != PENDING {
            return Err(ExecutionError::exception(
                ClrExceptionKind::InvalidOperation,
                "An attempt was made to transition a task to a final state when it had already completed.",
            ));
        }
        set_field(i, task, STATUS, Value::I32(COMPLETED));
        set_field(i, task, RESULT, result);
        Ok(take_continuations(i, task))
    })?;
    // Resuming runs managed code, which may await again — so it happens after
    // the lock is released, never under it.
    resume_all(interp, taken)
}

fn fault(interp: &mut Interpreter, task: Handle, exception: Value) -> ExecResult<()> {
    let taken = interp.interlocked(|i| {
        set_field(i, task, STATUS, Value::I32(FAULTED));
        set_field(i, task, EXCEPTION, exception);
        take_continuations(i, task)
    });
    resume_all(interp, taken)
}

/// Empties a task's waiting list and hands it back.
fn take_continuations(interp: &mut Interpreter, task: Handle) -> Vec<Value> {
    let list = field_handle(interp, task, CONTINUATIONS);
    if list.is_null() {
        return Vec::new();
    }
    let waiting = elements(interp, list);
    if !waiting.is_empty() {
        let fresh = interp.alloc_value_array(0);
        set_field(interp, task, CONTINUATIONS, Value::Obj(fresh));
    }
    waiting
}

/// Resumes every state machine that was waiting.
fn resume_all(interp: &mut Interpreter, waiting: Vec<Value>) -> ExecResult<()> {
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
        .heap.with::<ClrObject, _>(cell, |o| o.fields.first().cloned()).flatten()
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
    settle(interp, task)?;
    match status_of(interp, task) {
        FAULTED => {
            let exception = field(interp, task, EXCEPTION);
            match exception.as_handle().filter(|h| !h.is_null()) {
                // Rethrow the original instance, so `catch` sees the exception
                // the failing task actually threw rather than a stand-in.
                Some(object) => {
                    let message = interp
                        .heap.with::<rustclr_core::ClrException, _>(object, |e| e.message.clone())
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
    // `Task.Delay` hands back a *pending* task and arms a timer, so
    // `var d = Task.Delay(500); Work(); await d;` overlaps the two. Sleeping
    // the caller here instead would make every delay a stop.
    //
    // One timer thread serves every outstanding delay; the completion runs on a
    // pool worker like any other job.
    #[cfg(feature = "std")]
    interp.register_native(key(&format!("{TASKS}.Task"), "Delay(int)"), |i, a| {
        let ms = arg_i32(i, a, 0)?.max(0) as u64;
        let task = new_task(i, false);
        i.queue_work_after(core::time::Duration::from_millis(ms), Box::new(move |worker| {
            let _ = complete(worker, task, Value::Null);
        }));
        Ok(Some(Value::Obj(task)))
    });
    // Without threads there is nothing to overlap with, so the delay is a sleep
    // and the task comes back finished.
    #[cfg(not(feature = "std"))]
    interp.register_native(key(&format!("{TASKS}.Task"), "Delay(int)"), |i, a| {
        let ms = arg_i32(i, a, 0)?.max(0) as u64;
        sleep_millis(i, ms);
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
/// `Task.Run(() => …)` — starts the body on another thread.
///
/// The task is handed back pending, with the id of the thread running it. Every
/// way of observing a task — `Result`, `Wait`, `WaitAll`, an awaiter's
/// `IsCompleted` — settles it first, so the body has finished before anything
/// can see the answer.
///
/// This is real overlap between `Task.Run` and whatever the caller does next.
/// It is *not* `async`/`await` overlapping: an `async` method still runs its
/// body inline, and an `await` blocks rather than suspending. Starting several
/// tasks and awaiting them afterwards genuinely runs them at once; awaiting one
/// inside a loop does not.
#[cfg(feature = "std")]
fn run_delegate(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let body = args.first().cloned().unwrap_or(Value::Null);
    let task = new_task(interp, true);

    // Queued on the pool rather than given a thread of its own. A thousand
    // tasks used to mean a thousand OS threads, each paying for a copy of the
    // loader on the way up; now they share one worker per core.
    interp.queue_work(Box::new(move |worker| {
        let outcome = match invoke_delegate(worker, &body, &[]) {
            Ok(value) => {
                // `Task.Run(async () => …)` hands back a task, and .NET's
                // overload unwraps it: the caller gets `Task<int>`, not
                // `Task<Task<int>>`. Completing with the inner task itself
                // would give a caller reading `Result` the task object.
                match unwrap_inner_task(worker, value) {
                    Ok(value) => complete(worker, task, value),
                    Err(e) => {
                        let exception = exception_value(&e);
                        fault(worker, task, exception)
                    }
                }
            }
            // A failure inside the delegate becomes a faulted task rather than
            // escaping the worker, which is what `await` expects to observe.
            Err(e) => {
                let exception = exception_value(&e);
                fault(worker, task, exception)
            }
        };
        // Nothing above this can report an error anywhere useful — the caller
        // is elsewhere by now — and a task that ends up neither completed nor
        // faulted would hang whoever awaits it.
        if outcome.is_err() {
            let _ = fault(worker, task, Value::Null);
        }
    }));
    Ok(Some(Value::Obj(task)))
}

/// Without threads the body runs where it is created, as it always did.
#[cfg(not(feature = "std"))]
fn run_delegate(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let body = args.first().cloned().unwrap_or(Value::Null);
    match invoke_delegate(interp, &body, &[]) {
        Ok(value) => Ok(Some(Value::Obj(completed_task(interp, value)))),
        Err(e) => {
            let task = new_task(interp, true);
            let exception = exception_value(&e);
            fault(interp, task, exception)?;
            Ok(Some(Value::Obj(task)))
        }
    }
}

/// Whether the native being serviced returns nothing.
fn returns_void(interp: &Interpreter) -> bool {
    interp
        .current_native_method()
        .map(|m| interp.loader.registry.method(m).returns_void())
        .unwrap_or(false)
}

/// Unwraps the task an async delegate returned.
///
/// `Task.Run(Func<Task<T>>)` is `Unwrap`ped by .NET, so the outer task settles
/// with the inner one's *result*. Waiting for the inner task here is what makes
/// that possible, and it is safe on a pool worker because waiting helps run
/// queued work rather than idling.
#[cfg(feature = "std")]
fn unwrap_inner_task(interp: &mut Interpreter, value: Value) -> ExecResult<Value> {
    let Some(handle) = value.as_handle().filter(|h| !h.is_null()) else {
        return Ok(value);
    };
    if !is_task(interp, handle) {
        return Ok(value);
    }
    settle(interp, handle)?;
    result_of(interp, handle)
}

/// Waits for a task's body to finish, if one is running on another thread.
///
/// Two ways to wait, because the thread's join handle belongs to whichever
/// interpreter started it: the starter joins, and anyone else watches the
/// task's own status. Both are done from inside `blocking`, so a collection
/// does not sit waiting for a thread that is itself waiting.
#[cfg(feature = "std")]
fn settle(interp: &mut Interpreter, task: Handle) -> ExecResult<()> {
    if status_of(interp, task) != PENDING {
        return Ok(());
    }

    // A task started by `Task.Run` names the thread running it, and the thread
    // that started it can simply join.
    if let Some(id) = field(interp, task, THREAD).as_i64() {
        if interp.owns_thread(id as u64) {
            return interp.join(id as u64);
        }
    }

    // Otherwise it is completed by somebody else: a pool worker, another
    // thread, or — for the task an `async` method returns — whichever thread
    // finishes the inner task this one is suspended on.
    //
    // Waiting *helps*: a queued job runs here rather than this thread idling.
    // Without that a pool deadlocks in the classic way, every worker blocked on
    // a task whose job is still in the queue behind it.
    //
    // And it is bounded by whether anything can still complete the task. When
    // no work is outstanding and no other thread is running, nothing ever will,
    // and saying so beats hanging.
    let mut settled = false;
    while !settled {
        if status_of(interp, task) != PENDING {
            settled = true;
            break;
        }
        if interp.help_with_queued_work() {
            continue;
        }
        if !interp.work_may_still_arrive() {
            break;
        }
        settled = interp.blocking(|i| {
            // Re-check under the blocked state, then give the CPU up briefly.
            if status_of(i, task) != PENDING {
                return true;
            }
            std::thread::yield_now();
            status_of(i, task) != PENDING
        });
    }

    if settled {
        Ok(())
    } else {
        Err(ExecutionError::exception(
            ClrExceptionKind::InvalidOperation,
            "This task never completed, and no thread is left that could complete it.              An async method that suspends on something nothing finishes cannot make              progress. See docs/limitations.md.",
        ))
    }
}

#[cfg(not(feature = "std"))]
fn settle(_i: &mut Interpreter, _task: Handle) -> ExecResult<()> {
    Ok(())
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
        .heap.with::<ClrDelegate, _>(handle, |d| d.targets.clone())
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
    // An async *method*'s state machine is a struct, and the pointer has to go
    // straight through so the body's writes to its own fields land in the
    // caller's local. An async *iterator*'s is a class — it has to outlive the
    // call to be enumerated — and there `this` is the object itself. Passing a
    // pointer-to-local for one of those makes every `ldfld` in the body read
    // from the wrong thing, which shows up as a null receiver two frames later.
    let by_value = !interp.loader.registry.ty(type_id).kind.is_value_like();
    let receiver = if by_value { machine } else { machine_ref };
    interp.invoke(move_next, vec![receiver])?;
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

    // The state machine is copied onto the heap here, which is what makes an
    // `await` a suspension: it has to outlive the frame it was a local of.
    let object_type = interp.loader.core().object;
    let mut cell = ClrObject::new(object_type, 1);
    cell.fields[0] = machine;
    let cell = interp.heap.alloc(cell);

    // Checking the status and joining the queue happen together, against
    // another thread completing the task in between — see `complete`.
    let already_done = interp.interlocked(|i| {
        if status_of(i, task) != PENDING {
            return true;
        }
        let list = field_handle(i, task, CONTINUATIONS);
        if list.is_null() {
            let fresh = i.alloc_value_array(0);
            set_field(i, task, CONTINUATIONS, Value::Obj(fresh));
        }
        let list = field_handle(i, task, CONTINUATIONS);
        crate::collections::push_value(i, list, Value::Obj(cell));
        false
    });

    // Finished before we could queue: resume now rather than waiting for a
    // drain that has already happened. Outside the lock, as resuming runs
    // managed code.
    if already_done {
        resume(interp, cell)?;
    }
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
        "ValueTaskAwaiter",
        "ValueTaskAwaiter`1",
        "ConfiguredValueTaskAwaitable+ConfiguredValueTaskAwaiter",
        "ConfiguredValueTaskAwaitable`1+ConfiguredValueTaskAwaiter",
    ] {
        let full: &'static str = Box::leak(format!("{CS}.{name}").into_boxed_str());

        // An awaiter's `IsCompleted` is the gate `await` branches on, and it
        // answers honestly: `false` for a task still running.
        //
        // That is what makes `await` a suspension rather than a wait. The state
        // machine goes down `AwaitUnsafeOnCompleted`, which queues it on the
        // task and returns — so the async method returns a pending task to its
        // caller, and the thread completing the task runs the continuation.
        // Answering `true` here after blocking would be correct and would make
        // every `await` sequential.
        interp.register_native(key(full, "get_IsCompleted()"), |i, a| {
            let task = as_task(i, a, 0)?;
            Ok(Some(Value::I32((status_of(i, task) != PENDING) as i32)))
        });
        interp.register_native(key(full, "GetResult()"), |i, a| {
            let task = as_task(i, a, 0)?;
            let value = result_of(i, task)?;
            // A non-generic `TaskAwaiter.GetResult()` returns void, and handing
            // back a value for a void method leaves it on the evaluation stack
            // — everything the caller reads afterwards is then off by one. The
            // awaited task still has to be settled, so the work happens either
            // way and only the answer is dropped.
            if returns_void(i) {
                return Ok(None);
            }
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
        "ConfiguredValueTaskAwaitable",
        "ConfiguredValueTaskAwaitable`1",
    ] {
        let full: &'static str = Box::leak(format!("{CS}.{name}").into_boxed_str());
        interp.register_native(key(full, "GetAwaiter()"), |i, a| Ok(Some(arg(i, a, 0)?)));
    }
}

// -- ValueTask ---------------------------------------------------------------

/// A `ValueTask` is represented by the task it stands for.
///
/// .NET's `ValueTask` is a struct that holds *either* a result or a `Task`,
/// so that a method which usually completes synchronously need not allocate.
/// That is an allocation optimisation, and it is the one part of the type that
/// does not survive here: this runtime represents every `ValueTask` as a task,
/// so the allocation happens anyway. Everything observable — awaiting, results,
/// exceptions, `AsTask`, `IsCompleted` — is the same, and that is what a
/// program can actually tell apart.
///
/// `default(ValueTask)` is the case worth naming. A struct's default is all
/// zeroes, which arrives here as null, and in .NET it means an *already
/// successfully completed* task rather than an absent one. So null is read as
/// completed rather than refused, which is why these do not go through
/// `as_task`.
fn register_value_task(interp: &mut Interpreter) {
    for name in ["ValueTask", "ValueTask`1"] {
        let full: &'static str = Box::leak(format!("{TASKS}.{name}").into_boxed_str());

        // `new ValueTask(task)` and `new ValueTask<T>(result)`. The first hands
        // back the task; the second needs a completed task carrying the value.
        // The constructor writes *through* `this`, which is a managed pointer.
        //
        // Returning the value instead would serve `newobj` and nothing else:
        // `ValueTask<int> v = new ValueTask<int>(11)` does not compile to
        // `newobj` at all. Roslyn emits `ldloca v; ldc.i4 11; call .ctor`, so
        // the only way the local ever holds anything is a write through the
        // pointer. `newobj` reads the same slot back out of its cell, so one
        // shape covers both.
        for ctor in [".ctor/1", ".ctor/2"] {
            interp.register_native(key(full, ctor), |i, a| {
                let source = arg(i, a, 1)?;
                let task = match source.as_handle().filter(|h| !h.is_null()) {
                    // `new ValueTask(task)` stands for that task.
                    Some(h) if is_task(i, h) => h,
                    // `new ValueTask<T>(source, token)` — an `IValueTaskSource`.
                    // An async iterator's `MoveNextAsync` returns this shape,
                    // with the state machine as its own source.
                    //
                    // The result is read *now* rather than when the value task
                    // is awaited. That is sound only because nothing overlaps:
                    // the body has already run to the next `yield return`, so
                    // the promise is settled before the value task exists. On a
                    // runtime where it could still be pending this would be
                    // wrong, which is why it is written down here.
                    Some(h) if a.len() > 2 => {
                        let token = arg(i, a, 2)?;
                        let value = call_get_result(i, h, token)?;
                        let t = new_task(i, true);
                        complete(i, t, value)?;
                        t
                    }
                    // `new ValueTask<T>(result)` wraps a value already in hand.
                    _ => {
                        let t = new_task(i, true);
                        complete(i, t, source)?;
                        t
                    }
                };
                if let Some(Value::Ref(r)) = a.first() {
                    i.store_indirect_public(r.clone(), Value::Obj(task))?;
                }
                Ok(None)
            });
        }

        interp.register_native(key(full, "get_CompletedTask()"), |i, _a| {
            let task = new_task(i, false);
            complete(i, task, Value::Null)?;
            Ok(Some(Value::Obj(task)))
        });
        interp.register_native(key(full, "FromResult/1"), |i, a| {
            let task = new_task(i, true);
            let value = arg(i, a, 1).or_else(|_| arg(i, a, 0))?;
            complete(i, task, value)?;
            Ok(Some(Value::Obj(task)))
        });

        // The awaiter is the value task is the task.
        for member in ["GetAwaiter()", "ConfigureAwait(bool)", "ConfigureAwait/1", "AsTask()", "Preserve()"] {
            interp.register_native(key(full, member), |i, a| Ok(Some(arg(i, a, 0)?)));
        }

        interp.register_native(key(full, "get_IsCompleted()"), |i, a| {
            Ok(Some(Value::I32(completed_or_default(i, a)? as i32)))
        });
        interp.register_native(key(full, "get_IsCompletedSuccessfully()"), |i, a| {
            Ok(Some(Value::I32(completed_or_default(i, a)? as i32)))
        });
        interp.register_native(key(full, "get_IsFaulted()"), |i, a| {
            match value_task_handle(i, a)? {
                Some(t) => Ok(Some(Value::I32((status_of(i, t) == FAULTED) as i32))),
                None => Ok(Some(Value::I32(0))),
            }
        });
        interp.register_native(key(full, "get_IsCanceled()"), |_i, _a| Ok(Some(Value::I32(0))));
        interp.register_native(key(full, "get_Result()"), |i, a| match value_task_handle(i, a)? {
            Some(t) => result_of(i, t),
            None => Ok(Value::Null),
        }
        .map(Some));
    }
}

/// Calls `IValueTaskSource<T>.GetResult(token)` on an object.
///
/// The method is found on the object's own type by name and arity. An async
/// iterator implements the interface *explicitly*, so the name carries the
/// interface prefix — matching the suffix covers both spellings.
fn call_get_result(
    interp: &mut Interpreter,
    source: Handle,
    token: Value,
) -> ExecResult<Value> {
    let Some(ty) = interp.type_of(source) else {
        return Err(ExecutionError::null_reference());
    };
    let method = interp
        .loader
        .registry
        .base_chain(ty)
        .into_iter()
        .flat_map(|t| interp.loader.registry.ty(t).methods.clone())
        .find(|m| {
            let info = interp.loader.registry.method(*m);
            (info.name == "GetResult" || info.name.ends_with(".GetResult"))
                && info.signature.params.len() == 1
        })
        .ok_or_else(|| {
            ExecutionError::MissingImplementation(
                "a ValueTask source without GetResult(short)".into(),
            )
        })?;
    Ok(interp
        .invoke(method, alloc::vec![Value::Obj(source), token])?
        .unwrap_or(Value::Null))
}

/// Whether a handle names a `Task` rather than a result being wrapped.
fn is_task(interp: &mut Interpreter, handle: Handle) -> bool {
    let Some(actual) = interp.type_of(handle) else { return false };
    [task_type(interp, false), task_type(interp, true)]
        .into_iter()
        .flatten()
        .any(|t| t == actual)
}

/// The task behind a value task, or `None` for `default(ValueTask)`.
fn value_task_handle(
    interp: &mut Interpreter,
    args: &[Value],
) -> ExecResult<Option<Handle>> {
    let v = arg(interp, args, 0)?;
    Ok(v.as_handle().filter(|h| !h.is_null()))
}

/// `default(ValueTask)` is a completed task, so an absent one reads as done.
fn completed_or_default(interp: &mut Interpreter, args: &[Value]) -> ExecResult<bool> {
    Ok(match value_task_handle(interp, args)? {
        Some(t) => status_of(interp, t) != PENDING,
        None => true,
    })
}

// -- async iterators ---------------------------------------------------------

const SOURCES: &str = "System.Threading.Tasks.Sources";

#[cfg(feature = "std")]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "std")]
use std::sync::{Arc, Mutex};

/// Slots of `ManualResetValueTaskSourceCore<T>`.
const PROMISE_RESULT: usize = 0;
const PROMISE_HAS_RESULT: usize = 1;

/// `async IAsyncEnumerable<T>` and the `await foreach` that consumes it.
///
/// The compiler lowers an async iterator into a state machine that implements
/// `IAsyncEnumerable<T>`, `IAsyncEnumerator<T>` and `IValueTaskSource<bool>`
/// itself — all in IL, all of which this runtime already executes. Two pieces
/// are the runtime's to supply: the builder that drives `MoveNext`, and the
/// promise `MoveNextAsync` hands back.
///
/// Both are simple here for the same reason the rest of `async` is: nothing
/// overlaps. By the time `MoveNextAsync` returns, the body has already run to
/// the next `yield return`, so the promise always has its answer and the
/// "reset, await, complete later" cycle it exists to manage never happens.
fn register_async_iterators(interp: &mut Interpreter) {
    let builder: &'static str =
        Box::leak(format!("{CS}.AsyncIteratorMethodBuilder").into_boxed_str());

    // The builder holds nothing this runtime needs; a task stands in so that
    // `AwaitOnCompleted` can share the ordinary async path.
    interp.register_native(key(builder, "Create()"), |i, _a| {
        Ok(Some(Value::Obj(new_task(i, false))))
    });
    interp.register_native(key(builder, "MoveNext/1"), start_state_machine);
    interp.register_native(key(builder, "Complete()"), |_i, _a| Ok(None));
    interp.register_native(key(builder, "AwaitOnCompleted/2"), await_on_completed);
    interp.register_native(key(builder, "AwaitUnsafeOnCompleted/2"), await_on_completed);

    let promise: &'static str =
        Box::leak(format!("{SOURCES}.ManualResetValueTaskSourceCore`1").into_boxed_str());

    interp.register_native(key(promise, "SetResult(!0)"), |i, a| {
        let cell = promise_cell(i, a)?;
        let value = arg(i, a, 1)?;
        set_field(i, cell, PROMISE_RESULT, value);
        set_field(i, cell, PROMISE_HAS_RESULT, Value::I32(1));
        Ok(None)
    });
    interp.register_native(key(promise, "SetException/1"), |i, a| {
        // A faulting iterator: the promise is completed with the exception, and
        // since nothing overlaps the consumer is already waiting for it.
        let cell = promise_cell(i, a)?;
        let exception = arg(i, a, 1)?;
        set_field(i, cell, PROMISE_RESULT, exception);
        set_field(i, cell, PROMISE_HAS_RESULT, Value::I32(1));
        Ok(None)
    });
    interp.register_native(key(promise, "GetResult/1"), |i, a| {
        let cell = promise_cell(i, a)?;
        Ok(Some(field(i, cell, PROMISE_RESULT)))
    });
    interp.register_native(key(promise, "GetStatus/1"), |i, a| {
        let cell = promise_cell(i, a)?;
        // 0 Pending, 1 Succeeded — the only two this runtime produces.
        let done = field(i, cell, PROMISE_HAS_RESULT).as_i32().unwrap_or(0);
        Ok(Some(Value::I32(done)))
    });
    interp.register_native(key(promise, "OnCompleted/4"), |_i, _a| Ok(None));
    interp.register_native(key(promise, "Reset()"), |i, a| {
        let cell = promise_cell(i, a)?;
        set_field(i, cell, PROMISE_RESULT, Value::Null);
        set_field(i, cell, PROMISE_HAS_RESULT, Value::I32(0));
        Ok(None)
    });
    interp.register_native(key(promise, "get_Version()"), |_i, _a| Ok(Some(Value::I32(0))));

    // The `ValueTask` awaiters, registered *after* the shared awaiter loop so
    // these win. The shared ones refuse a null receiver, which is right for a
    // `TaskAwaiter` and wrong here: `default(ValueTask)` is all zeroes, and an
    // async iterator's `DisposeAsync` returns exactly that once the enumeration
    // has finished. Refusing it ends every `await foreach` with a null
    // reference at the closing brace.
    for name in [
        "ValueTaskAwaiter",
        "ValueTaskAwaiter`1",
        "ConfiguredValueTaskAwaitable+ConfiguredValueTaskAwaiter",
        "ConfiguredValueTaskAwaitable`1+ConfiguredValueTaskAwaiter",
    ] {
        let full: &'static str = Box::leak(format!("{CS}.{name}").into_boxed_str());
        interp.register_native(key(full, "get_IsCompleted()"), |i, a| {
            Ok(Some(Value::I32(completed_or_default(i, a)? as i32)))
        });
        interp.register_native(key(full, "GetResult()"), |i, a| {
            match value_task_handle(i, a)? {
                Some(t) => result_of(i, t),
                None => Ok(Value::Null),
            }
            .map(Some)
        });
    }
}

/// The storage behind a `ManualResetValueTaskSourceCore<T>` field.
///
/// The core is a struct living in a field of the state machine, reached by
/// `ldflda`, so `this` is a managed pointer. Its default is all zeroes, which
/// arrives as null — so the first access allocates the storage and writes it
/// back through the pointer, exactly as the interpolated string handler does.
fn promise_cell(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Handle> {
    if let Some(h) = arg(interp, args, 0)?.as_handle().filter(|h| !h.is_null()) {
        return Ok(h);
    }
    let Some(type_id) = interp
        .loader
        .registry
        .find_type_by_name(&format!("{SOURCES}.ManualResetValueTaskSourceCore`1"))
    else {
        return Err(ExecutionError::MissingImplementation(
            "ManualResetValueTaskSourceCore is not registered".into(),
        ));
    };
    let cell = interp.alloc_object(type_id);
    set_field(interp, cell, PROMISE_HAS_RESULT, Value::I32(0));
    match args.first() {
        Some(Value::Ref(r)) => {
            interp.store_indirect_public(r.clone(), Value::Obj(cell))?;
            Ok(cell)
        }
        // Passed by value, so the allocation could not be written back and
        // every later call would get a fresh one. Refuse rather than lose it.
        _ => Err(ExecutionError::InvalidProgram(
            "a ValueTask source must be reached by reference".into(),
        )),
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
