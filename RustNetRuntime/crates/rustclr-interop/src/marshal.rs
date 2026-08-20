//! Marshalling between managed values and the native ABI.
//!
//! Calling an arbitrary C function whose signature is only known at runtime
//! normally needs libffi. This module takes a different route: it classifies
//! each argument into one of two ABI slots — integer/pointer or floating point
//! — and dispatches through a table of concrete function types. That covers
//! the shapes P/Invoke declarations overwhelmingly use, and every unsupported
//! shape is *rejected* rather than guessed at, so a mismatched call surfaces as
//! an error instead of corrupting the stack.

use rustclr_core::{ClrString, ExecutionError, Interpreter, Value};
use std::ffi::CString;

/// How one argument is passed to native code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiSlot {
    /// Integers, booleans, chars and pointers.
    Integer,
    /// `float` and `double`.
    Float,
}

/// A marshalled argument plus anything that must stay alive for the call.
pub struct Marshalled {
    pub slot: AbiSlot,
    pub integer: i64,
    pub float: f64,
    /// Keeps a converted string alive until the call returns.
    _owned: Option<CString>,
}

impl Marshalled {
    fn integer(v: i64) -> Self {
        Self { slot: AbiSlot::Integer, integer: v, float: 0.0, _owned: None }
    }

    fn float(v: f64) -> Self {
        Self { slot: AbiSlot::Float, integer: 0, float: v, _owned: None }
    }

    fn pointer(v: i64, owned: CString) -> Self {
        Self { slot: AbiSlot::Integer, integer: v, float: 0.0, _owned: Some(owned) }
    }
}

/// The native return shape a P/Invoke declaration expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnShape {
    Void,
    Int32,
    Int64,
    Double,
    /// A pointer that should be read back as a NUL-terminated string.
    CString,
    /// A raw pointer handed back to managed code as `native int`.
    Pointer,
}

/// Converts a managed value into its native representation.
///
/// Strings are marshalled as UTF-8 `char*`, which is what modern
/// cross-platform P/Invoke declarations expect. Wide-string marshalling is not
/// yet implemented and is reported rather than silently mis-encoded.
pub fn marshal_argument(
    interp: &Interpreter,
    value: &Value,
) -> Result<Marshalled, ExecutionError> {
    Ok(match value {
        Value::I32(v) => Marshalled::integer(*v as i64),
        Value::I64(v) | Value::NativeInt(v) => Marshalled::integer(*v),
        Value::F(v) => Marshalled::float(*v),
        Value::Null => Marshalled::integer(0),
        Value::Obj(h) => match interp.heap.get_as::<ClrString>(*h) {
            Some(s) => {
                let text = s.to_rust_string();
                let c = CString::new(text).map_err(|_| {
                    ExecutionError::exception(
                        rustclr_core::ClrExceptionKind::Argument,
                        "A marshalled string may not contain an embedded NUL.",
                    )
                })?;
                let ptr = c.as_ptr() as i64;
                Marshalled::pointer(ptr, c)
            }
            None => {
                return Err(ExecutionError::Unsupported(
                    "only strings and blittable primitives can cross the interop boundary"
                        .into(),
                ))
            }
        },
        other => {
            return Err(ExecutionError::Unsupported(format!(
                "cannot marshal a {} to native code",
                other.kind_name()
            )))
        }
    })
}

/// Reads a NUL-terminated UTF-8 string returned by native code.
///
/// # Safety
///
/// `ptr` must be a valid pointer to a NUL-terminated buffer owned by the callee
/// and must remain valid for the duration of this call.
pub unsafe fn read_c_string(interp: &mut Interpreter, ptr: i64) -> Value {
    if ptr == 0 {
        return Value::Null;
    }
    let text = unsafe { std::ffi::CStr::from_ptr(ptr as *const i8) }
        .to_string_lossy()
        .into_owned();
    Value::Obj(interp.alloc_string(&text))
}

/// Infers the return shape from a method's managed signature.
pub fn return_shape(sig: &rustclr_core::metadata::MethodSig) -> ReturnShape {
    use rustclr_core::metadata::TypeSig;
    match sig.return_type.unwrap_modifiers() {
        TypeSig::Void => ReturnShape::Void,
        TypeSig::R4 | TypeSig::R8 => ReturnShape::Double,
        TypeSig::I8 | TypeSig::U8 => ReturnShape::Int64,
        TypeSig::IntPtr | TypeSig::UIntPtr | TypeSig::Ptr(_) => ReturnShape::Pointer,
        TypeSig::String => ReturnShape::CString,
        _ => ReturnShape::Int32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclr_core::{CaptureHost, Interpreter};

    fn interp() -> Interpreter {
        Interpreter::with_host(Box::new(CaptureHost::new()))
    }

    #[test]
    fn integers_and_floats_take_different_abi_slots() {
        let i = interp();
        assert_eq!(marshal_argument(&i, &Value::I32(7)).unwrap().slot, AbiSlot::Integer);
        assert_eq!(marshal_argument(&i, &Value::F(1.5)).unwrap().slot, AbiSlot::Float);
        assert_eq!(marshal_argument(&i, &Value::Null).unwrap().integer, 0);
    }

    #[test]
    fn strings_marshal_as_a_pointer_that_stays_alive() {
        let mut i = interp();
        let h = i.alloc_string("hello");
        let m = marshal_argument(&i, &Value::Obj(h)).unwrap();
        assert_eq!(m.slot, AbiSlot::Integer);
        assert_ne!(m.integer, 0);
        // Reading it back through the pointer must give the same text.
        let text = unsafe { std::ffi::CStr::from_ptr(m.integer as *const i8) };
        assert_eq!(text.to_str().unwrap(), "hello");
    }

    #[test]
    fn a_non_blittable_object_is_rejected_rather_than_guessed_at() {
        let mut i = interp();
        let array = i.alloc_array(i.loader.core().object, 2);
        assert!(marshal_argument(&i, &Value::Obj(array)).is_err());
    }
}
