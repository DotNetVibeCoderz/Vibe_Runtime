//! `CancellationTokenSource` and `CancellationToken`.
//!
//! A source is an object with two slots: whether it has been cancelled, and a
//! value array of the callbacks registered on its tokens, stored as
//! (delegate, state) pairs. A token is a value type whose single slot holds
//! its source, or nothing for `CancellationToken.None`. Cancelling runs the
//! callbacks newest first on the cancelling thread, as .NET does.
//!
//! Registering and cancelling can happen on different threads at once, so the
//! check-and-add and the set-and-take are each one step under the runtime's
//! lock; the callbacks themselves run after it is released. A registration is
//! the list slot it occupies, and disposing it empties that slot -- without
//! that, every wait on a long-lived token would leave a callback behind.

use crate::collections::{field, set_field};
use crate::support::*;
use rustclr_core::{ClrArray, ClrDelegate, ExecResult, ExecutionError, Interpreter, Value};
use rustclr_gc::Handle;

#[allow(unused_imports)]
use crate::prelude::*;

const SOURCE: &str = "System.Threading.CancellationTokenSource";
const TOKEN: &str = "System.Threading.CancellationToken";
const CANCELED: usize = 0;
const CALLBACKS: usize = 1;

fn key(ty: &str, member: &str) -> &'static str {
    Box::leak(format!("{ty}::{member}").into_boxed_str())
}

/// The source behind a token or a source receiver; null for `None`.
fn source_of(i: &mut Interpreter, a: &[Value]) -> ExecResult<Handle> {
    Ok(arg(i, a, 0)?.as_handle().unwrap_or(Handle::NULL))
}

fn is_canceled(i: &mut Interpreter, source: Handle) -> bool {
    !source.is_null() && field(i, source, CANCELED).is_truthy()
}

fn callbacks(i: &mut Interpreter, source: Handle) -> Handle {
    let existing = field(i, source, CALLBACKS).as_handle().filter(|h| !h.is_null());
    match existing {
        Some(h) => h,
        None => {
            let array = i.alloc_value_array(0);
            set_field(i, source, CALLBACKS, Value::Obj(array));
            array
        }
    }
}

/// Calls a callback delegate, passing the state when the target takes one.
fn call(i: &mut Interpreter, delegate: &Value, state: &Value) -> ExecResult<()> {
    let Some(handle) = delegate.as_handle().filter(|h| !h.is_null()) else { return Ok(()) };
    let targets = i.heap.with::<ClrDelegate, _>(handle, |d| d.targets.clone()).unwrap_or_default();
    for target in targets {
        let mut args = Vec::with_capacity(2);
        if !target.receiver.is_null() {
            args.push(Value::Obj(target.receiver));
        }
        if !i.loader.registry.method(target.method).signature.params.is_empty() {
            args.push(state.clone());
        }
        i.invoke(target.method, args)?;
    }
    Ok(())
}

/// A list entry that cancels the task in its state slot.
const CANCEL_TASK: i32 = 1;
/// A list entry that cancels the linked source in its state slot.
const CANCEL_SOURCE: i32 = 2;

fn cancel(i: &mut Interpreter, source: Handle) -> ExecResult<()> {
    if source.is_null() {
        return Ok(());
    }
    // Set and take together, against a registration racing in: it either
    // lands in the list before this takes it, or sees the source cancelled
    // and runs its callback itself.
    let pending = i.interlocked(|i| {
        if is_canceled(i, source) {
            return None;
        }
        set_field(i, source, CANCELED, Value::I32(1));
        let list = callbacks(i, source);
        Some(
            i.heap
                .with_mut::<ClrArray, _>(list, |a| match &mut a.storage {
                    rustclr_core::ArrayStorage::Values(v) => core::mem::take(v),
                    _ => Vec::new(),
                })
                .unwrap_or_default(),
        )
    });
    let Some(pending) = pending else { return Ok(()) };
    // The list is off the source now, so it is rooted here while callbacks run.
    i.with_roots(&pending, |i| {
        let mut first_error = None;
        // Last registered runs first, as on .NET.
        for pair in pending.chunks(2).rev() {
            if let [entry, state] = pair {
                if let Err(e) = run(i, entry, state) {
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

/// Runs one entry: a delegate, or one of the native kinds.
fn run(i: &mut Interpreter, entry: &Value, state: &Value) -> ExecResult<()> {
    match entry {
        Value::I32(CANCEL_TASK) => {
            if let Some(task) = state.as_handle().filter(|h| !h.is_null()) {
                crate::tasks::cancel_task(i, task)?;
            }
            Ok(())
        }
        Value::I32(CANCEL_SOURCE) => {
            if let Some(child) = state.as_handle().filter(|h| !h.is_null()) {
                cancel(i, child)?;
            }
            Ok(())
        }
        // A disposed registration leaves its slot empty.
        Value::Null => Ok(()),
        delegate => call(i, delegate, state),
    }
}

/// Adds an entry, or runs it at once if the source is already cancelled.
/// Returns the registration: the slot it occupies, or null if it ran.
fn add(i: &mut Interpreter, source: Handle, entry: Value, state: Value) -> ExecResult<Value> {
    if source.is_null() {
        return Ok(Value::Null);
    }
    let slot = i.interlocked(|i| {
        if is_canceled(i, source) {
            return None;
        }
        let list = callbacks(i, source);
        i.heap
            .with_mut::<ClrArray, _>(list, |a| match &mut a.storage {
                rustclr_core::ArrayStorage::Values(v) => {
                    v.push(entry.clone());
                    v.push(state.clone());
                    Some(v.len() - 2)
                }
                _ => None,
            })
            .flatten()
    });
    match slot {
        Some(index) => {
            let registration = i.alloc_value_array(2);
            crate::collections::with_values(i, registration, |v| {
                v[0] = Value::Obj(source);
                v[1] = Value::I32(index as i32);
            });
            Ok(Value::Obj(registration))
        }
        None => {
            run(i, &entry, &state)?;
            Ok(Value::Null)
        }
    }
}

/// Empties the slot a registration occupies, if the source still has it.
fn unregister(i: &mut Interpreter, registration: &Value) -> bool {
    let Some(reg) = registration.as_handle().filter(|h| !h.is_null()) else { return false };
    let parts = crate::collections::elements(i, reg);
    let (Some(source), Some(index)) = (
        parts.first().and_then(|v| v.as_handle()).filter(|h| !h.is_null()),
        parts.get(1).and_then(|v| v.as_i32()),
    ) else {
        return false;
    };
    i.interlocked(|i| {
        if is_canceled(i, source) {
            return false;
        }
        let list = callbacks(i, source);
        i.heap
            .with_mut::<ClrArray, _>(list, |a| match &mut a.storage {
                rustclr_core::ArrayStorage::Values(v) => {
                    let index = index as usize;
                    if index + 1 < v.len() && !matches!(v[index], Value::Null) {
                        v[index] = Value::Null;
                        v[index + 1] = Value::Null;
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            })
            .unwrap_or(false)
    })
}

fn register_callback(i: &mut Interpreter, a: &[Value], with_state: bool) -> ExecResult<Option<Value>> {
    let source = source_of(i, a)?;
    let delegate = arg(i, a, 1)?;
    let state = if with_state { arg(i, a, 2)? } else { Value::Null };
    Ok(Some(add(i, source, delegate, state)?))
}

/// The source a token value stands for; null for `None` and `default`.
fn token_source(token: &Value) -> Handle {
    token.as_handle().unwrap_or(Handle::NULL)
}

/// Whether a token argument has been cancelled.
pub(crate) fn token_is_canceled(i: &mut Interpreter, token: &Value) -> bool {
    let source = token_source(token);
    is_canceled(i, source)
}

/// Arranges for `task` to be cancelled when `token` is. A token that cannot
/// be cancelled arranges nothing.
pub(crate) fn cancel_task_on(i: &mut Interpreter, token: &Value, task: Handle) -> ExecResult<()> {
    let source = token_source(token);
    add(i, source, Value::I32(CANCEL_TASK), Value::Obj(task)).map(|_| ())
}

/// Cancels `source` after `millis`, on the pool's timer.
#[cfg(feature = "threads")]
fn cancel_after(i: &mut Interpreter, source: Handle, millis: u64) {
    crate::tasks::queue_rooted(i, core::time::Duration::from_millis(millis), &[source], move |worker| {
        let _ = cancel(worker, source);
    });
}

#[cfg(not(feature = "threads"))]
fn cancel_after(i: &mut Interpreter, source: Handle, millis: u64) {
    // Nothing runs concurrently without threads, so a deadline cannot arrive
    // while the program runs. Cancelling at once would be wrong for a long
    // deadline; ignoring it is what a deadline that never fires looks like.
    let _ = (i, source, millis);
}

/// A `CancelAfter` / timed-constructor argument: `int` milliseconds or a
/// `TimeSpan` in ticks. Negative is infinite.
fn delay_millis(value: &Value) -> Option<u64> {
    let millis = match value {
        Value::I64(ticks) => *ticks / 10_000,
        other => other.as_i32().map(i64::from).unwrap_or(-1),
    };
    (millis >= 0).then_some(millis as u64)
}

fn new_source(i: &mut Interpreter) -> ExecResult<Handle> {
    let Some(ty) = i.loader.registry.find_type_by_name(SOURCE) else {
        return Err(ExecutionError::MissingImplementation(
            "CancellationTokenSource is not registered".into(),
        ));
    };
    let source = i.alloc_object(ty);
    set_field(i, source, CANCELED, Value::I32(0));
    Ok(source)
}

/// Cancels at once for a zero delay, arms a timer for a positive one, and
/// does nothing for an infinite one.
fn arm(i: &mut Interpreter, source: Handle, delay: &Value) -> ExecResult<()> {
    match delay_millis(delay) {
        Some(0) => cancel(i, source),
        Some(ms) => {
            cancel_after(i, source, ms);
            Ok(())
        }
        None => Ok(()),
    }
}

pub fn register(interp: &mut Interpreter) {
    for ctor in [".ctor()", ".ctor/0"] {
        interp.register_native(key(SOURCE, ctor), |i, a| {
            if let Some(source) = arg(i, a, 0)?.as_handle().filter(|h| !h.is_null()) {
                set_field(i, source, CANCELED, Value::I32(0));
            }
            Ok(None)
        });
    }
    // `new CancellationTokenSource(delay)`: cancels itself after the delay.
    interp.register_native(key(SOURCE, ".ctor/1"), |i, a| {
        let source = arg_handle(i, a, 0)?;
        set_field(i, source, CANCELED, Value::I32(0));
        let delay = arg(i, a, 1)?;
        arm(i, source, &delay)?;
        Ok(None)
    });
    interp.register_native(key(SOURCE, "CancelAfter/1"), |i, a| {
        let source = source_of(i, a)?;
        let delay = arg(i, a, 1)?;
        arm(i, source, &delay)?;
        Ok(None)
    });
    interp.register_native(key(SOURCE, "get_Token()"), |i, a| Ok(Some(Value::Obj(source_of(i, a)?))));
    interp.register_native(key(SOURCE, "get_IsCancellationRequested()"), |i, a| {
        let s = source_of(i, a)?;
        Ok(Some(Value::I32(is_canceled(i, s) as i32)))
    });
    for name in ["Cancel()", "Cancel/1"] {
        interp.register_native(key(SOURCE, name), |i, a| {
            let s = source_of(i, a)?;
            cancel(i, s)?;
            Ok(None)
        });
    }
    interp.register_native(key(SOURCE, "Dispose()"), |_i, _a| Ok(None));
    interp.register_native(key(SOURCE, "TryReset()"), |i, a| {
        let s = source_of(i, a)?;
        Ok(Some(Value::I32(!is_canceled(i, s) as i32)))
    });
    // `CreateLinkedTokenSource(a, b)` and the `params` / span overloads: a new
    // source that each parent cancels on its way down.
    for member in ["CreateLinkedTokenSource/1", "CreateLinkedTokenSource/2"] {
        interp.register_native(key(SOURCE, member), |i, a| {
            let child = new_source(i)?;
            let mut parents = Vec::new();
            for n in 0..a.len() {
                let value = arg(i, a, n)?;
                let is_array = value
                    .as_handle()
                    .filter(|h| !h.is_null())
                    .is_some_and(|h| i.heap.with::<ClrArray, _>(h, |_| ()).is_some());
                if is_array {
                    let h = value.as_handle().unwrap_or(Handle::NULL);
                    parents.extend(crate::collections::elements(i, h));
                } else if let Some(values) = crate::spans::values(i, &value) {
                    parents.extend(values);
                } else {
                    parents.push(value);
                }
            }
            i.with_roots(&[Value::Obj(child)], |i| {
                for parent in parents {
                    add(i, token_source(&parent), Value::I32(CANCEL_SOURCE), Value::Obj(child))?;
                }
                Ok::<_, ExecutionError>(())
            })?;
            Ok(Some(Value::Obj(child)))
        });
    }

    interp.register_native(key(TOKEN, "get_None()"), |_i, _a| Ok(Some(Value::I32(0))));
    // `new CancellationToken(canceled)`: a token with a source of its own.
    interp.register_native(key(TOKEN, ".ctor/1"), |i, a| {
        let canceled = arg(i, a, 1)?.is_truthy();
        let source = new_source(i)?;
        if canceled {
            set_field(i, source, CANCELED, Value::I32(1));
        }
        if let Some(Value::Ref(r)) = a.first() {
            i.store_indirect_public(r.clone(), Value::Obj(source))?;
        }
        Ok(None)
    });
    interp.register_native(key(TOKEN, "get_IsCancellationRequested()"), |i, a| {
        let s = source_of(i, a)?;
        Ok(Some(Value::I32(is_canceled(i, s) as i32)))
    });
    interp.register_native(key(TOKEN, "get_CanBeCanceled()"), |i, a| {
        let s = source_of(i, a)?;
        Ok(Some(Value::I32(!s.is_null() as i32)))
    });
    interp.register_native(key(TOKEN, "ThrowIfCancellationRequested()"), |i, a| {
        let s = source_of(i, a)?;
        if is_canceled(i, s) {
            return Err(ExecutionError::exception(
                rustclr_core::ClrExceptionKind::Managed("System.OperationCanceledException".into()),
                "The operation was canceled.",
            ));
        }
        Ok(None)
    });
    // Two tokens are equal when they come from one source; every token that
    // cannot be cancelled is `CancellationToken.None`, so those are all equal.
    interp.register_native(key(TOKEN, "Equals/1"), |i, a| {
        let x = source_of(i, a)?;
        let y = token_source(&arg(i, a, 1)?);
        Ok(Some(Value::I32((x == y) as i32)))
    });
    interp.register_native(key(TOKEN, "op_Equality/2"), |i, a| {
        let x = token_source(&arg(i, a, 0)?);
        let y = token_source(&arg(i, a, 1)?);
        Ok(Some(Value::I32((x == y) as i32)))
    });
    interp.register_native(key(TOKEN, "op_Inequality/2"), |i, a| {
        let x = token_source(&arg(i, a, 0)?);
        let y = token_source(&arg(i, a, 1)?);
        Ok(Some(Value::I32((x != y) as i32)))
    });
    interp.register_native(key(TOKEN, "Register/1"), |i, a| register_callback(i, a, false));
    interp.register_native(key(TOKEN, "Register/2"), |i, a| register_callback(i, a, true));
    interp.register_native(key(TOKEN, "UnsafeRegister/2"), |i, a| register_callback(i, a, true));
    interp.register_native("System.Threading.CancellationTokenRegistration::Dispose()", |i, a| {
        let registration = arg(i, a, 0)?;
        unregister(i, &registration);
        Ok(None)
    });
    interp.register_native("System.Threading.CancellationTokenRegistration::Unregister()", |i, a| {
        let registration = arg(i, a, 0)?;
        Ok(Some(Value::I32(unregister(i, &registration) as i32)))
    });
}
