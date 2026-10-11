//! `CancellationTokenSource` and `CancellationToken`.
//!
//! A source is an object with two slots: whether it has been cancelled, and a
//! value array of the callbacks registered on its tokens, stored as
//! (delegate, state) pairs. A token is a value type whose single slot holds
//! its source, or nothing for `CancellationToken.None`. Cancelling runs the
//! callbacks newest first on the cancelling thread, as .NET does.

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

fn cancel(i: &mut Interpreter, source: Handle) -> ExecResult<()> {
    if source.is_null() || is_canceled(i, source) {
        return Ok(());
    }
    set_field(i, source, CANCELED, Value::I32(1));
    let list = callbacks(i, source);
    let pending = i
        .heap
        .with_mut::<ClrArray, _>(list, |a| match &mut a.storage {
            rustclr_core::ArrayStorage::Values(v) => core::mem::take(v),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    // Last registered runs first, as on .NET.
    for pair in pending.chunks(2).rev() {
        if let [delegate, state] = pair {
            call(i, delegate, state)?;
        }
    }
    Ok(())
}

fn register_callback(i: &mut Interpreter, a: &[Value], with_state: bool) -> ExecResult<Option<Value>> {
    let source = source_of(i, a)?;
    let delegate = arg(i, a, 1)?;
    let state = if with_state { arg(i, a, 2)? } else { Value::Null };
    if source.is_null() {
        return Ok(Some(Value::I32(0)));
    }
    if is_canceled(i, source) {
        call(i, &delegate, &state)?;
    } else {
        let list = callbacks(i, source);
        i.heap.with_mut::<ClrArray, _>(list, |a| {
            if let rustclr_core::ArrayStorage::Values(v) = &mut a.storage {
                v.push(delegate);
                v.push(state);
            }
        });
    }
    Ok(Some(Value::I32(0)))
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

    interp.register_native(key(TOKEN, "get_None()"), |_i, _a| Ok(Some(Value::I32(0))));
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
    interp.register_native(key(TOKEN, "Register/1"), |i, a| register_callback(i, a, false));
    interp.register_native(key(TOKEN, "Register/2"), |i, a| register_callback(i, a, true));
    interp.register_native("System.Threading.CancellationTokenRegistration::Dispose()", |_i, _a| Ok(None));
}
