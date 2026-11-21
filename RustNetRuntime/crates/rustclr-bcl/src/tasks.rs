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

use crate::collections::{elements, field, field_handle, sequence_values, set_field};
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
/// There are no OS threads without `std`, so nothing reads it there.
#[cfg(feature = "threads")]
const THREAD: usize = 4;
/// The `TaskCreationOptions` a completion source asked for.
const OPTIONS: usize = 5;
/// `TaskCreationOptions.RunContinuationsAsynchronously`.
const RUN_CONTINUATIONS_ASYNCHRONOUSLY: i32 = 64;

const PENDING: i32 = 0;
const COMPLETED: i32 = 1;
const FAULTED: i32 = 2;
/// Cancelled: `EXCEPTION` holds the `TaskCanceledException` an `await` throws.
const CANCELED: i32 = 3;

/// A waiting-list entry that is not a suspended state machine.
///
/// The list holds state-machine cells (ordinary objects) for `await`, and
/// these — a small array whose first element says which — for the other ways
/// of waiting on a task without blocking a thread.
const ENTRY_CONTINUE_WITH: i32 = 1;
const ENTRY_WHEN_ANY: i32 = 2;

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
#[cfg(feature = "threads")]
fn degree(items: usize) -> usize {
    let cores = rustclr_thread::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
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
#[cfg(feature = "threads")]
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
        interp.blocking(|_| rustclr_thread::thread::yield_now());
    }

    let error = outcome.lock().expect("parallel outcome poisoned").take();
    match error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// `Parallel.Invoke`: one thread per action.
#[cfg(feature = "threads")]
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
        interp.blocking(|_| rustclr_thread::thread::yield_now());
    }
    let error = outcome.lock().expect("parallel outcome poisoned").take();
    match error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Without `std` there is one thread, so a parallel loop is a loop.
#[cfg(not(feature = "threads"))]
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

#[cfg(not(feature = "threads"))]
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
    // Literal names: this runs for every task, and building the name with
    // `format!` was an allocation per task for a string that never changes.
    let name = if generic { "System.Threading.Tasks.Task`1" } else { "System.Threading.Tasks.Task" };
    interp.loader.registry.find_type_by_name(name)
}

/// Allocates a task in the pending state.
///
/// One allocation, fields written before it is published. A fresh object's
/// fields are null, and null already means what each slot needs: `status_of`
/// reads a null status as pending, and a null waiting list as empty — the
/// list is made by the first `await`, which most tasks from `Task.Run` never
/// see. This used to be two allocations and four separate heap locks.
fn new_task(interp: &mut Interpreter, generic: bool) -> Handle {
    let Some(type_id) = task_type(interp, generic) else { return Handle::NULL };
    let mut object = ClrObject::new(type_id, 6);
    object.fields[STATUS] = Value::I32(PENDING);
    interp.alloc_clr_object(object)
}

/// A task already carrying its result.
pub(crate) fn completed_task(interp: &mut Interpreter, result: Value) -> Handle {
    let generic = !matches!(result, Value::Null);
    let handle = new_task(interp, generic);
    set_field(interp, handle, RESULT, result);
    set_field(interp, handle, STATUS, Value::I32(COMPLETED));
    handle
}

fn status_of(interp: &Interpreter, task: Handle) -> i32 {
    field(interp, task, STATUS).as_i32().unwrap_or(PENDING)
}

/// Marks a task finished and resumes everything waiting on it.
fn complete(interp: &mut Interpreter, task: Handle, result: Value) -> ExecResult<()> {
    if transition(interp, task, COMPLETED, result)? {
        Ok(())
    } else {
        Err(already_final())
    }
}

fn already_final() -> ExecutionError {
    ExecutionError::exception(
        ClrExceptionKind::InvalidOperation,
        "An attempt was made to transition a task to a final state when it had already completed.",
    )
}

/// Faults a task that is still pending; one already final is left alone.
fn fault(interp: &mut Interpreter, task: Handle, exception: Value) -> ExecResult<()> {
    transition(interp, task, FAULTED, exception).map(|_| ())
}

/// Cancels a task that is still pending. Reports whether it did.
fn cancel(interp: &mut Interpreter, task: Handle) -> ExecResult<bool> {
    if status_of(interp, task) != PENDING {
        return Ok(false);
    }
    // Allocated before the lock: allocating can collect, and a collection
    // must not start while this thread holds the task lock.
    let exception = new_exception(
        interp,
        "System.Threading.Tasks.TaskCanceledException",
        "A task was canceled.",
        Handle::NULL,
    );
    interp.with_roots(&[Value::Obj(exception)], |i| {
        transition(i, task, CANCELED, Value::Obj(exception))
    })
}

/// Moves a pending task to a final state and resumes what was waiting on it.
///
/// Answers `false`, changing nothing, when the task was already final — which
/// is what every `TrySet…` reports and what a racing completion must see.
///
/// Settling the status and taking the waiting list happen together, against
/// `add_continuation` doing its check-and-add. Otherwise a task that finishes
/// between another thread's "is it pending?" and its "then wait for it" loses
/// that continuation, and the `await` never resumes.
///
/// `task` and `payload` are rooted across the lock. Taking it can mean
/// waiting, a thread that waits is one a collection proceeds without, and a
/// payload made by the caller — a fresh `TaskCanceledException`, a result
/// object — is otherwise held only by this Rust frame.
fn transition(interp: &mut Interpreter, task: Handle, status: i32, payload: Value) -> ExecResult<bool> {
    let rooted = [Value::Obj(task), payload.clone()];
    // Under one heap lock, which is what `add_continuation` takes too: the two
    // cannot interleave, so a continuation is either added before this takes
    // the list or sees the task final and runs itself.
    let taken = interp.with_roots(&rooted, |interp| interp.heap.atomically(|heap| {
        let object = heap.get_as_mut::<ClrObject>(task)?;
        if object.fields.get(STATUS).and_then(|v| v.as_i32()).unwrap_or(PENDING) != PENDING {
            return None;
        }
        // The payload first, the status last. A reader checks the status
        // without this lock — `IsCompleted`, `settle` — and then reads the
        // result, so a status published before its result let `.Result` see a
        // finished task holding nothing: a ping-pong benchmark printed 0.
        let slot = if status == COMPLETED { RESULT } else { EXCEPTION };
        object.fields[slot] = payload;
        object.fields[STATUS] = Value::I32(status);
        let list = core::mem::replace(&mut object.fields[CONTINUATIONS], Value::Null);
        let waiting = match list.as_handle().filter(|h| !h.is_null()) {
            Some(list) => heap
                .get_as::<rustclr_core::ClrArray>(list)
                .map(|a| (0..a.len()).filter_map(|n| a.storage.get(n)).collect())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        Some(waiting)
    }));
    match taken {
        // Resuming runs managed code, which may await again — so it happens
        // after the lock is released, never under it.
        Some(waiting) if !waiting.is_empty() && runs_continuations_asynchronously(interp, task) => {
            resume_elsewhere(interp, task, waiting);
            Ok(true)
        }
        Some(waiting) => {
            resume_all(interp, task, waiting)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

fn runs_continuations_asynchronously(interp: &Interpreter, task: Handle) -> bool {
    field(interp, task, OPTIONS).as_i32().unwrap_or(0) & RUN_CONTINUATIONS_ASYNCHRONOUSLY != 0
}

/// Resumes the waiters on a pool worker instead of the completing thread.
///
/// What `RunContinuationsAsynchronously` asks for, and what it is for: the
/// thread that completes a source may be holding a lock, and a continuation
/// run inline would run *under* it — re-entrantly, since `Monitor` is
/// re-entrant, so the lock would exclude nothing.
#[cfg(feature = "threads")]
fn resume_elsewhere(interp: &mut Interpreter, task: Handle, waiting: Vec<Value>) {
    let mut held: Vec<Handle> = waiting.iter().filter_map(|v| v.as_handle()).filter(|h| !h.is_null()).collect();
    held.push(task);
    queue_rooted(interp, core::time::Duration::ZERO, &held, move |worker| {
        let _ = resume_all(worker, task, waiting);
    });
}

/// With one thread there is nowhere else to run them.
#[cfg(not(feature = "threads"))]
fn resume_elsewhere(interp: &mut Interpreter, task: Handle, waiting: Vec<Value>) {
    let _ = resume_all(interp, task, waiting);
}

/// An exception object of a framework type, built without throwing it.
fn new_exception(interp: &mut Interpreter, type_name: &str, message: &str, inner: Handle) -> Handle {
    let mut error = ExecutionError::exception(ClrExceptionKind::Managed(type_name.into()), message);
    let handle = interp.materialise_exception(&mut error);
    if !inner.is_null() {
        interp.heap.with_mut::<rustclr_core::ClrException, _>(handle, |e| e.inner = inner);
    }
    handle
}

/// `AggregateException` around one exception, as `Wait` and `Result` throw.
fn aggregate(interp: &mut Interpreter, inner: Handle) -> Handle {
    // Either shape: a runtime-raised `ClrException`, or an object `newobj`
    // built, which keeps its message in its first field.
    let message = crate::runtime::exception_message(interp, inner);
    let text = format!("One or more errors occurred. ({message})");
    new_exception(interp, "System.AggregateException", &text, inner)
}

/// An error carrying an exception object that already exists.
fn rethrow(interp: &mut Interpreter, object: Handle) -> ExecutionError {
    let message = crate::runtime::exception_message(interp, object);
    ExecutionError::Exception {
        kind: ClrExceptionKind::Managed(interp.type_name_of(object)),
        message,
        object,
    }
}

/// Adds an entry to a pending task's waiting list.
///
/// Answers `true` when the task had already finished, in which case nothing
/// was added and the caller runs the entry itself — outside the lock, since
/// that runs managed code.
///
/// The entry is rooted across the lock, for the reason `transition` gives:
/// an `await` boxes its state machine into a fresh cell just before this, and
/// until the cell is on the task's list nothing else holds it. A collection
/// run by another thread while this one queued for the lock freed it, and the
/// method then never resumed — or resumed into whatever reused the slot.
fn add_continuation(interp: &mut Interpreter, task: Handle, entry: Value) -> bool {
    if status_of(interp, task) != PENDING {
        return true;
    }
    // A list is made outside the lock, which must not allocate, and only when
    // the task has none yet. Another thread may make one at the same moment;
    // whichever is installed first is used, and the other is garbage.
    let fresh = if field_handle(interp, task, CONTINUATIONS).is_null() {
        interp.alloc_value_array(0)
    } else {
        Handle::NULL
    };
    let rooted = [Value::Obj(task), entry.clone(), Value::Obj(fresh)];
    interp.with_roots(&rooted, |interp| interp.heap.atomically(|heap| {
        let Some(object) = heap.get_as_mut::<ClrObject>(task) else { return true };
        if object.fields.get(STATUS).and_then(|v| v.as_i32()).unwrap_or(PENDING) != PENDING {
            return true;
        }
        let list = match object.fields[CONTINUATIONS].as_handle().filter(|h| !h.is_null()) {
            Some(list) => list,
            None if !fresh.is_null() => {
                object.fields[CONTINUATIONS] = Value::Obj(fresh);
                fresh
            }
            // Pending, with no list, and none made: the list was taken by a
            // completion that then — impossibly — left the task pending.
            // Refusing to lose the entry, the caller runs it.
            None => return true,
        };
        match heap.get_as_mut::<rustclr_core::ClrArray>(list).and_then(|a| a.storage.values_mut()) {
            Some(values) => {
                values.push(entry);
                false
            }
            None => true,
        }
    }))
}

/// Resumes everything that was waiting on `task`.
///
/// The list was taken off the task, so until each entry runs it is reachable
/// only from this Rust local — and running the first one runs managed code,
/// which can collect. Rooting the whole list for the duration is what keeps
/// the second entry alive while the first resumes.
fn resume_all(interp: &mut Interpreter, task: Handle, waiting: Vec<Value>) -> ExecResult<()> {
    if waiting.is_empty() {
        return Ok(());
    }
    let mut roots = waiting.clone();
    roots.push(Value::Obj(task));
    interp.with_roots(&roots, |i| {
        let mut first_error = None;
        for entry in waiting {
            if let Some(handle) = entry.as_handle().filter(|h| !h.is_null()) {
                // One continuation failing must not strand the others: each is
                // somebody's `await`, and skipping it hangs that method.
                if let Err(e) = run_entry(i, task, handle) {
                    first_error.get_or_insert(e);
                }
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    })
}

/// Runs one waiting-list entry for a task that has just become final.
fn run_entry(interp: &mut Interpreter, task: Handle, entry: Handle) -> ExecResult<()> {
    let parts = interp.heap.with::<rustclr_core::ClrArray, _>(entry, |a| {
        (0..a.len()).filter_map(|n| a.storage.get(n)).collect::<Vec<Value>>()
    });
    let Some(parts) = parts else {
        // Not an array: a suspended state machine.
        return resume(interp, entry);
    };
    match parts.first().and_then(|v| v.as_i32()) {
        Some(ENTRY_CONTINUE_WITH) => {
            let body = parts.get(1).cloned().unwrap_or(Value::Null);
            let result = parts.get(2).and_then(|v| v.as_handle()).unwrap_or(Handle::NULL);
            match invoke_delegate(interp, &body, &[Value::Obj(task)]) {
                Ok(value) => {
                    let _ = transition(interp, result, COMPLETED, value)?;
                }
                Err(e) => {
                    let exception = exception_value(&e);
                    fault(interp, result, exception)?;
                }
            }
            Ok(())
        }
        Some(ENTRY_WHEN_ANY) => {
            // The first of the set to finish wins; the rest find it final.
            let result = parts.get(1).and_then(|v| v.as_handle()).unwrap_or(Handle::NULL);
            let _ = transition(interp, result, COMPLETED, Value::Obj(task))?;
            Ok(())
        }
        _ => Ok(()),
    }
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
    // A struct machine (release builds) resumes through a pointer at the
    // cell's slot, so its own `stfld` writes land in the copy that will be
    // resumed next time. A class machine (debug builds) is its own `this`, as
    // in `start_state_machine`; a pointer to it reads every field as null.
    let this = if interp.loader.registry.ty(type_id).kind.is_value_like() {
        Value::Ref(rustclr_core::ByRef::Field { object: cell, slot: 0 })
    } else {
        machine
    };
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

/// Reads a finished task's value the way `await` does: a fault rethrows the
/// original exception, a cancellation throws `TaskCanceledException`.
fn result_of(interp: &mut Interpreter, task: Handle) -> ExecResult<Value> {
    observe(interp, task, false)
}

/// Reads a finished task's value the way `Wait()` and `Result` do: the
/// exception arrives wrapped in an `AggregateException`.
///
/// The two differ on .NET and code relies on the difference — a `catch
/// (AggregateException)` around `.Wait()` catches nothing if the inner
/// exception arrives bare.
fn result_of_blocking(interp: &mut Interpreter, task: Handle) -> ExecResult<Value> {
    observe(interp, task, true)
}

fn observe(interp: &mut Interpreter, task: Handle, wrapped: bool) -> ExecResult<Value> {
    settle(interp, task)?;
    match status_of(interp, task) {
        FAULTED | CANCELED => {
            let exception = field(interp, task, EXCEPTION);
            match exception.as_handle().filter(|h| !h.is_null()) {
                // Rethrow the original instance, so `catch` sees the exception
                // the failing task actually threw rather than a stand-in.
                Some(object) if wrapped => {
                    let outer = aggregate(interp, object);
                    Err(rethrow(interp, outer))
                }
                Some(object) => Err(rethrow(interp, object)),
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
            Ok(Some(result_of_blocking(i, task)?))
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
        interp.register_native(key(name, "get_IsCanceled()"), |i, a| {
            let task = as_task(i, a, 0)?;
            Ok(Some(Value::I32((status_of(i, task) == CANCELED) as i32)))
        });
        // `Exception` is the `AggregateException` for a fault and null for a
        // cancellation — a cancelled task has not *failed*.
        interp.register_native(key(name, "get_Exception()"), |i, a| {
            let task = as_task(i, a, 0)?;
            if status_of(i, task) != FAULTED {
                return Ok(Some(Value::Null));
            }
            match field(i, task, EXCEPTION).as_handle().filter(|h| !h.is_null()) {
                Some(inner) => Ok(Some(Value::Obj(aggregate(i, inner)))),
                None => Ok(Some(Value::Null)),
            }
        });
        // `TaskStatus`: RanToCompletion 5, Canceled 6, Faulted 7. A pending
        // task reports WaitingForActivation (1), which is what .NET reports for
        // a promise-style task — an `async` method or a completion source. A
        // `Task.Run` body already running would say Running (3) there.
        interp.register_native(key(name, "get_Status()"), |i, a| {
            let task = as_task(i, a, 0)?;
            Ok(Some(Value::I32(match status_of(i, task) {
                COMPLETED => 5,
                CANCELED => 6,
                FAULTED => 7,
                _ => 1,
            })))
        });
        interp.register_native(key(name, "Wait()"), |i, a| {
            let task = as_task(i, a, 0)?;
            result_of_blocking(i, task)?;
            Ok(None)
        });
        // `Wait(milliseconds)` and `Wait(TimeSpan)` give up at the deadline
        // and say so; only a task that *finished* rethrows.
        for member in ["Wait(int)", "Wait(System.TimeSpan)"] {
            interp.register_native(key(name, member), |i, a| {
                let task = as_task(i, a, 0)?;
                let millis = timeout_millis(i, a, 1)?;
                if !settle_within(i, task, millis)? {
                    return Ok(Some(Value::I32(0)));
                }
                result_of_blocking(i, task)?;
                Ok(Some(Value::I32(1)))
            });
        }
        // `ContinueWith(Action<Task>)` and `ContinueWith(Func<Task, T>)`, plus
        // the overloads adding a token, options or scheduler — which run the
        // continuation the same way, since this runtime has one scheduler.
        for member in ["ContinueWith/1", "ContinueWith/2", "ContinueWith/3", "ContinueWith/4"] {
            interp.register_native(key(name, member), continue_with);
        }
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
    #[cfg(feature = "threads")]
    interp.register_native(key(&format!("{TASKS}.Task"), "Delay(int)"), |i, a| {
        let ms = arg_i32(i, a, 0)?.max(0) as u64;
        let task = new_task(i, false);
        delay_then(i, ms, task);
        Ok(Some(Value::Obj(task)))
    });
    // Without threads there is nothing to overlap with, so the delay is a sleep
    // and the task comes back finished.
    #[cfg(not(feature = "threads"))]
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
        // `WhenAll<T>` yields a `T[]`, which the caller indexes with `ldelem`
        // and measures with `ldlen`; a list there is a null reference.
        let element = i
            .current_native_method()
            .and_then(|m| i.loader.registry.method(m).generic_args.first().copied())
            .unwrap_or(i.loader.core().object);
        let array = i.alloc_array(element, results.len());
        i.heap.with_mut::<rustclr_core::ClrArray, _>(array, |arr| {
            for (n, v) in results.iter().enumerate() {
                arr.storage.set(n, v);
            }
        });
        Ok(Some(Value::Obj(completed_task(i, Value::Obj(array)))))
    });
    // `WhenAny` hands back a task that completes with whichever of the set
    // finishes first. It does not wait: each task gets a waiting-list entry
    // pointing at the result, and the first to finish completes it.
    for member in ["WhenAny/1", "WhenAny/2"] {
        interp.register_native(key(&format!("{TASKS}.Task"), member), |i, a| {
            let mut tasks = Vec::new();
            for n in 0..a.len() {
                let value = arg(i, a, n)?;
                match value.as_handle().filter(|h| !h.is_null()) {
                    Some(h) if is_task(i, h) => tasks.push(Value::Obj(h)),
                    // An array, a list or a span of tasks.
                    _ => tasks.extend(awaited_tasks(i, &value)),
                }
            }
            if tasks.is_empty() {
                return Err(ExecutionError::exception(
                    ClrExceptionKind::Argument,
                    "The tasks argument contained no tasks.",
                ));
            }
            let result = new_task(i, true);
            i.with_roots(&[Value::Obj(result)], |i| {
                for t in &tasks {
                    let Some(h) = t.as_handle().filter(|h| !h.is_null()) else { continue };
                    let entry = i.alloc_value_array(2);
                    crate::collections::with_values(i, entry, |v| {
                        v[0] = Value::I32(ENTRY_WHEN_ANY);
                        v[1] = Value::Obj(result);
                    });
                    if add_continuation(i, h, Value::Obj(entry)) {
                        // Already finished: it wins, unless an earlier one did.
                        let _ = transition(i, result, COMPLETED, Value::Obj(h))?;
                        break;
                    }
                }
                Ok::<_, ExecutionError>(())
            })?;
            Ok(Some(Value::Obj(result)))
        });
    }
    for member in [
        "Delay(System.TimeSpan)",
        "Delay(int,System.Threading.CancellationToken)",
        "Delay(System.TimeSpan,System.Threading.CancellationToken)",
    ] {
        interp.register_native(key(&format!("{TASKS}.Task"), member), delay_with);
    }
}

/// `Task.Delay` with a `TimeSpan` or a token.
///
/// The token is honoured: cancelling it cancels the delay at once rather than
/// at the deadline, which is what a program cancelling a wait is asking for.
fn delay_with(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let ms = timeout_millis(interp, args, 0)?;
    let token = if args.len() > 1 { arg(interp, args, 1)? } else { Value::Null };
    let task = new_task(interp, false);
    if crate::cancellation::token_is_canceled(interp, &token) {
        cancel(interp, task)?;
        return Ok(Some(Value::Obj(task)));
    }
    crate::cancellation::cancel_task_on(interp, &token, task)?;
    if let Some(ms) = ms {
        delay_then(interp, ms, task);
    }
    Ok(Some(Value::Obj(task)))
}

#[cfg(feature = "threads")]
fn delay_then(interp: &mut Interpreter, ms: u64, task: Handle) {
    queue_rooted(interp, core::time::Duration::from_millis(ms), &[task], move |worker| {
        let _ = transition(worker, task, COMPLETED, Value::Null);
    });
}

#[cfg(not(feature = "threads"))]
fn delay_then(interp: &mut Interpreter, ms: u64, task: Handle) {
    sleep_millis(interp, ms);
    let _ = transition(interp, task, COMPLETED, Value::Null);
}

/// A timeout argument in milliseconds: an `int`, or a `TimeSpan` (ticks).
/// `None` is infinite (`-1`, `Timeout.Infinite`, `Timeout.InfiniteTimeSpan`).
fn timeout_millis(interp: &mut Interpreter, args: &[Value], index: usize) -> ExecResult<Option<u64>> {
    let value = arg(interp, args, index)?;
    // A `TimeSpan` is ticks in a 64-bit slot; an `int` is milliseconds in a
    // 32-bit one. The slot's shape says which overload this is.
    let millis = match value {
        Value::I64(ticks) => {
            if ticks < 0 {
                -1
            } else {
                ticks / 10_000
            }
        }
        other => other.as_i32().map(i64::from).unwrap_or(0),
    };
    if millis < 0 {
        return Ok(None);
    }
    Ok(Some(millis as u64))
}

/// `ContinueWith`: a new task that runs `body(antecedent)` once the
/// antecedent is final, however it got there.
fn continue_with(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let antecedent = as_task(interp, args, 0)?;
    let body = arg(interp, args, 1)?;
    let result = new_task(interp, true);
    let entry = interp.with_roots(&[Value::Obj(result), body.clone()], |i| {
        let entry = i.alloc_value_array(3);
        crate::collections::with_values(i, entry, |v| {
            v[0] = Value::I32(ENTRY_CONTINUE_WITH);
            v[1] = body.clone();
            v[2] = Value::Obj(result);
        });
        entry
    });
    if add_continuation(interp, antecedent, Value::Obj(entry)) {
        interp.with_roots(&[Value::Obj(entry), Value::Obj(result)], |i| run_entry(i, antecedent, entry))?;
    }
    Ok(Some(Value::Obj(result)))
}

/// Queues work for after `delay`, keeping `handles` alive until it has run.
///
/// A queued job holds its handles in a Rust closure, which no collection can
/// see — so without a pin, an object reachable only from a job still waiting
/// in the queue is swept, and the job finds a dead handle when it runs.
#[cfg(feature = "threads")]
pub(crate) fn queue_rooted(
    interp: &mut Interpreter,
    delay: core::time::Duration,
    handles: &[Handle],
    job: impl FnOnce(&mut Interpreter) + Send + 'static,
) {
    interp.heap.pin_all(handles);
    let pinned: Vec<Handle> = handles.to_vec();
    let run = Box::new(move |worker: &mut Interpreter| {
        job(worker);
        worker.heap.unpin_all(&pinned);
    });
    if delay.is_zero() {
        interp.queue_work(run);
    } else {
        interp.queue_work_after(delay, run);
    }
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
#[cfg(feature = "threads")]
fn run_delegate(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let body = args.first().cloned().unwrap_or(Value::Null);
    let task = new_task(interp, true);

    // Queued on the pool rather than given a thread of its own. A thousand
    // tasks used to mean a thousand OS threads, each paying for a copy of the
    // loader on the way up; now they share one worker per core.
    // The task and the delegate are pinned while queued: the closure is the
    // only thing holding the delegate, and nothing a collection can see.
    let mut held = alloc::vec![task];
    if let Some(h) = body.as_handle().filter(|h| !h.is_null()) {
        held.push(h);
    }
    queue_rooted(interp, core::time::Duration::ZERO, &held, move |worker| {
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
    });
    Ok(Some(Value::Obj(task)))
}

/// Without threads the body runs where it is created, as it always did.
#[cfg(not(feature = "threads"))]
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
#[cfg(feature = "threads")]
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
#[cfg(feature = "threads")]
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
    //
    // "Nothing can complete it" has to be *persistent* before it is believed.
    // The counters it reads are sampled without a lock, and a job that has
    // been taken off the queue but has not yet completed its task leaves a
    // window where they say idle and a completion is still coming. Treating
    // one such sample as final turned a slow task into a spurious "this task
    // never completed" — rare, and only under load.
    const IDLE_SAMPLES_BEFORE_GIVING_UP: u32 = 50;
    let mut idle = 0;
    let mut settled = false;
    while !settled {
        if status_of(interp, task) != PENDING {
            settled = true;
            break;
        }
        if interp.help_with_queued_work() {
            idle = 0;
            continue;
        }
        if interp.work_may_still_arrive() {
            idle = 0;
        } else {
            idle += 1;
            if idle >= IDLE_SAMPLES_BEFORE_GIVING_UP {
                break;
            }
        }
        settled = interp.blocking(|i| {
            if status_of(i, task) != PENDING {
                return true;
            }
            // A yield rather than a sleep: the common case is a task about to
            // finish on another thread, and the samples above only need to be
            // separated, not spaced out.
            rustclr_thread::thread::yield_now();
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

#[cfg(not(feature = "threads"))]
fn settle(_i: &mut Interpreter, _task: Handle) -> ExecResult<()> {
    Ok(())
}

/// Waits up to `millis` (forever for `None`) for a task to become final.
/// Answers whether it did.
///
/// Unlike [`settle`] this never concludes that nothing can complete the task:
/// a bounded wait is allowed to wait for something that never happens, and
/// the answer then is `false`, not an error.
#[cfg(feature = "threads")]
fn settle_within(interp: &mut Interpreter, task: Handle, millis: Option<u64>) -> ExecResult<bool> {
    let Some(millis) = millis else {
        settle(interp, task)?;
        return Ok(true);
    };
    let deadline = rustclr_thread::time::Instant::now() + core::time::Duration::from_millis(millis);
    loop {
        if status_of(interp, task) != PENDING {
            return Ok(true);
        }
        if rustclr_thread::time::Instant::now() >= deadline {
            return Ok(false);
        }
        if interp.help_with_queued_work() {
            continue;
        }
        interp.blocking(|_| rustclr_thread::thread::sleep(core::time::Duration::from_millis(1)));
    }
}

#[cfg(not(feature = "threads"))]
fn settle_within(interp: &mut Interpreter, task: Handle, _millis: Option<u64>) -> ExecResult<bool> {
    Ok(status_of(interp, task) != PENDING)
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
    let cell = interp.alloc_clr_object(cell);

    // Checking the status and joining the queue happen together, against
    // another thread completing the task in between — see `transition`.
    let already_done = add_continuation(interp, task, Value::Obj(cell));

    // Finished before we could queue: resume now rather than waiting for a
    // drain that has already happened. Outside the lock, as resuming runs
    // managed code — and rooted, since the cell is on no list.
    if already_done {
        interp.with_roots(&[Value::Obj(cell)], |i| resume(i, cell))?;
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
                    // with the state machine as its own source. Settled, it
                    // is read now; pending, it is waited for — see
                    // `task_for_source`.
                    Some(h) if a.len() > 2 => {
                        let token = arg(i, a, 2)?;
                        task_for_source(i, h, token)?
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
            Ok(Some(Value::I32(match value_task_handle(i, a)? {
                Some(t) => (status_of(i, t) == COMPLETED) as i32,
                None => 1,
            })))
        });
        interp.register_native(key(full, "get_IsFaulted()"), |i, a| {
            match value_task_handle(i, a)? {
                Some(t) => Ok(Some(Value::I32((status_of(i, t) == FAULTED) as i32))),
                None => Ok(Some(Value::I32(0))),
            }
        });
        interp.register_native(key(full, "get_IsCanceled()"), |i, a| {
            match value_task_handle(i, a)? {
                Some(t) => Ok(Some(Value::I32((status_of(i, t) == CANCELED) as i32))),
                None => Ok(Some(Value::I32(0))),
            }
        });
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

#[cfg(feature = "threads")]
use core::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "threads")]
use rustclr_thread::sync::{Arc, Mutex};

/// Slots of `ManualResetValueTaskSourceCore<T>`.
const PROMISE_RESULT: usize = 0;
/// `ValueTaskSourceStatus`: 0 pending, 1 succeeded, 2 faulted.
const PROMISE_HAS_RESULT: usize = 1;
/// (continuation, state) pairs waiting for the promise.
const PROMISE_WAITERS: usize = 2;

const PROMISE_PENDING: i32 = 0;
const PROMISE_SUCCEEDED: i32 = 1;
const PROMISE_FAULTED: i32 = 2;

/// `async IAsyncEnumerable<T>` and the `await foreach` that consumes it.
///
/// The compiler lowers an async iterator into a state machine that implements
/// `IAsyncEnumerable<T>`, `IAsyncEnumerator<T>` and `IValueTaskSource<bool>`
/// itself — all in IL, all of which this runtime already executes. Two pieces
/// are the runtime's to supply: the builder that drives `MoveNext`, and the
/// promise `MoveNextAsync` hands back.
///
/// The promise is usually settled by the time `MoveNextAsync` returns — the
/// body ran to its next `yield return` — and then the value task is built
/// already complete. But an iterator that awaits something slow (a channel
/// with nothing in it yet) suspends *inside* `MoveNextAsync`, and the promise
/// is genuinely pending. Then the value task is a pending task registered as a
/// waiter, and settling the promise completes it.
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
        settle_promise(i, cell, PROMISE_SUCCEEDED, value)?;
        Ok(None)
    });
    interp.register_native(key(promise, "SetException/1"), |i, a| {
        let cell = promise_cell(i, a)?;
        let exception = arg(i, a, 1)?;
        settle_promise(i, cell, PROMISE_FAULTED, exception)?;
        Ok(None)
    });
    interp.register_native(key(promise, "GetResult/1"), |i, a| {
        let cell = promise_cell(i, a)?;
        let value = field(i, cell, PROMISE_RESULT);
        if field(i, cell, PROMISE_HAS_RESULT).as_i32() == Some(PROMISE_FAULTED) {
            if let Some(object) = value.as_handle().filter(|h| !h.is_null()) {
                return Err(rethrow(i, object));
            }
        }
        Ok(Some(value))
    });
    interp.register_native(key(promise, "GetStatus/1"), |i, a| {
        let cell = promise_cell(i, a)?;
        let status = field(i, cell, PROMISE_HAS_RESULT).as_i32().unwrap_or(PROMISE_PENDING);
        Ok(Some(Value::I32(status)))
    });
    // `OnCompleted(continuation, state, token, flags)`. The runtime's own
    // value tasks register with no continuation and their pending task as the
    // state; anything else registers a delegate, called with the state.
    interp.register_native(key(promise, "OnCompleted/4"), |i, a| {
        let cell = promise_cell(i, a)?;
        let continuation = arg(i, a, 1)?;
        let state = arg(i, a, 2)?;
        let settled = i.interlocked(|i| {
            if field(i, cell, PROMISE_HAS_RESULT).as_i32().unwrap_or(0) != PROMISE_PENDING {
                return true;
            }
            let list = match field(i, cell, PROMISE_WAITERS).as_handle().filter(|h| !h.is_null()) {
                Some(h) => h,
                None => {
                    let h = i.alloc_value_array(0);
                    set_field(i, cell, PROMISE_WAITERS, Value::Obj(h));
                    h
                }
            };
            crate::collections::push_value(i, list, continuation.clone());
            crate::collections::push_value(i, list, state.clone());
            false
        });
        if settled {
            wake_promise_waiter(i, cell, &continuation, &state)?;
        }
        Ok(None)
    });
    interp.register_native(key(promise, "Reset()"), |i, a| {
        let cell = promise_cell(i, a)?;
        set_field(i, cell, PROMISE_RESULT, Value::Null);
        set_field(i, cell, PROMISE_HAS_RESULT, Value::I32(PROMISE_PENDING));
        set_field(i, cell, PROMISE_WAITERS, Value::Null);
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

/// Settles a promise and wakes whoever registered while it was pending.
fn settle_promise(interp: &mut Interpreter, cell: Handle, status: i32, payload: Value) -> ExecResult<()> {
    // Rooted across the lock, as in `transition`.
    let rooted = [Value::Obj(cell), payload.clone()];
    let waiting = interp.with_roots(&rooted, |interp| interp.interlocked(|i| {
        set_field(i, cell, PROMISE_RESULT, payload);
        set_field(i, cell, PROMISE_HAS_RESULT, Value::I32(status));
        let list = field(i, cell, PROMISE_WAITERS).as_handle().filter(|h| !h.is_null());
        set_field(i, cell, PROMISE_WAITERS, Value::Null);
        list.map(|h| elements(i, h)).unwrap_or_default()
    }));
    if waiting.is_empty() {
        return Ok(());
    }
    interp.with_roots(&waiting, |i| {
        for pair in waiting.chunks(2) {
            if let [continuation, state] = pair {
                wake_promise_waiter(i, cell, continuation, state)?;
            }
        }
        Ok(())
    })
}

/// Hands a settled promise's outcome to one waiter.
fn wake_promise_waiter(interp: &mut Interpreter, cell: Handle, continuation: &Value, state: &Value) -> ExecResult<()> {
    if continuation.as_handle().filter(|h| !h.is_null()).is_some() {
        // A delegate: `Action<object>`, called with its state.
        invoke_delegate(interp, continuation, &[state.clone()])?;
        return Ok(());
    }
    let Some(task) = state.as_handle().filter(|h| !h.is_null()) else { return Ok(()) };
    let value = field(interp, cell, PROMISE_RESULT);
    if field(interp, cell, PROMISE_HAS_RESULT).as_i32() == Some(PROMISE_FAULTED) {
        fault(interp, task, value)
    } else {
        transition(interp, task, COMPLETED, value).map(|_| ())
    }
}

/// Calls a method of a value-task source by name and arity.
///
/// The compiler implements `IValueTaskSource` *explicitly* on an async
/// iterator, so the name carries the interface prefix — matching the suffix
/// covers both spellings.
fn call_source(
    interp: &mut Interpreter,
    source: Handle,
    name: &str,
    args: Vec<Value>,
) -> ExecResult<Option<Value>> {
    let Some(method) = source_method(interp, source, name, args.len())? else { return Ok(None) };
    let mut call = alloc::vec![Value::Obj(source)];
    call.extend(args);
    interp.invoke(method, call)
}

/// The source's implementation of `name`, explicit or not.
///
/// Separate from [`call_source`] because `OnCompleted` returns `void`: a call
/// that ran and a method that does not exist both come back as `None`, and
/// treating the first as the second refused every pending source.
fn source_method(
    interp: &mut Interpreter,
    source: Handle,
    name: &str,
    arity: usize,
) -> ExecResult<Option<rustclr_core::MethodId>> {
    let Some(ty) = interp.type_of(source) else {
        return Err(ExecutionError::null_reference());
    };
    let suffix = format!(".{name}");
    Ok(interp
        .loader
        .registry
        .base_chain(ty)
        .into_iter()
        .flat_map(|t| interp.loader.registry.ty(t).methods.clone())
        .find(|m| {
            let info = interp.loader.registry.method(*m);
            (info.name == name || info.name.ends_with(&suffix)) && info.signature.params.len() == arity
        }))
}

/// The value task for `new ValueTask<T>(source, token)`.
///
/// Settled sources become a finished task now. A pending one becomes a
/// pending task, registered on the source so settling it completes the task.
fn task_for_source(interp: &mut Interpreter, source: Handle, token: Value) -> ExecResult<Handle> {
    let status = call_source(interp, source, "GetStatus", alloc::vec![token.clone()])?
        .and_then(|v| v.as_i32())
        .unwrap_or(PROMISE_SUCCEEDED);
    let task = new_task(interp, true);
    interp.with_roots(&[Value::Obj(task), Value::Obj(source)], |i| {
        if status == PROMISE_PENDING {
            if source_method(i, source, "OnCompleted", 4)?.is_none() {
                return Err(ExecutionError::MissingImplementation(
                    "a pending ValueTask source without OnCompleted".into(),
                ));
            }
            call_source(
                i,
                source,
                "OnCompleted",
                alloc::vec![Value::Null, Value::Obj(task), token.clone(), Value::I32(0)],
            )?;
            return Ok(());
        }
        match call_get_result(i, source, token.clone()) {
            Ok(value) => complete(i, task, value),
            Err(e) => {
                let exception = exception_value(&e);
                if exception.as_handle().filter(|h| !h.is_null()).is_some() {
                    fault(i, task, exception)
                } else {
                    Err(e)
                }
            }
        }
    })?;
    Ok(task)
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

        // `new TaskCompletionSource()`, `(TaskCreationOptions)`, `(object
        // state)` and `(state, options)`. Only the options change behaviour,
        // and an `int` argument is how they arrive.
        for ctor in [".ctor()", ".ctor/0", ".ctor/1", ".ctor/2"] {
            interp.register_native(key(full, ctor), |i, a| {
                let this = arg_handle(i, a, 0)?;
                let task = new_task(i, true);
                let options = a
                    .iter()
                    .skip(1)
                    .filter_map(|v| match v {
                        Value::I32(n) => Some(*n),
                        _ => None,
                    })
                    .last()
                    .unwrap_or(0);
                set_field(i, task, OPTIONS, Value::I32(options));
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
        // The `TrySet…` family is one atomic step: two threads racing to
        // complete the same source get one `true` between them.
        interp.register_native(key(full, "TrySetResult/1"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let task = field_handle(i, this, 0);
            let value = arg(i, a, 1)?;
            Ok(Some(Value::I32(transition(i, task, COMPLETED, value)? as i32)))
        });
        interp.register_native(key(full, "TrySetException/1"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let task = field_handle(i, this, 0);
            let exception = arg(i, a, 1)?;
            Ok(Some(Value::I32(transition(i, task, FAULTED, exception)? as i32)))
        });
        interp.register_native(key(full, "TrySetResult()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let task = field_handle(i, this, 0);
            Ok(Some(Value::I32(transition(i, task, COMPLETED, Value::Null)? as i32)))
        });
        for member in ["TrySetCanceled()", "TrySetCanceled/1"] {
            interp.register_native(key(full, member), |i, a| {
                let this = arg_handle(i, a, 0)?;
                let task = field_handle(i, this, 0);
                Ok(Some(Value::I32(cancel(i, task)? as i32)))
            });
        }
        for member in ["SetCanceled()", "SetCanceled/1"] {
            interp.register_native(key(full, member), |i, a| {
                let this = arg_handle(i, a, 0)?;
                let task = field_handle(i, this, 0);
                if cancel(i, task)? {
                    Ok(None)
                } else {
                    Err(already_final())
                }
            });
        }
        interp.register_native(key(full, "SetException/1"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let task = field_handle(i, this, 0);
            let exception = arg(i, a, 1)?;
            if transition(i, task, FAULTED, exception)? {
                Ok(None)
            } else {
                Err(already_final())
            }
        });
    }
}

/// Completes `task` as cancelled. For `cancellation`, which owns the tokens.
pub(crate) fn cancel_task(interp: &mut Interpreter, task: Handle) -> ExecResult<bool> {
    cancel(interp, task)
}
