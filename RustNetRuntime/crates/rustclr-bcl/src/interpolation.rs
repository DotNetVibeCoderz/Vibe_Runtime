//! `System.Runtime.CompilerServices.DefaultInterpolatedStringHandler`.
//!
//! Since C# 10 an interpolated string is not compiled to `String.Format`. The
//! compiler emits a handler struct on the stack and a sequence of calls:
//!
//! ```text
//! ldloca handler
//! ldc.i4 <literal length>; ldc.i4 <hole count>
//! call instance void DefaultInterpolatedStringHandler::.ctor(int32, int32)
//! ldloca handler; ldstr "Found "; call AppendLiteral(string)
//! ldloca handler; ldloc count;    call AppendFormatted<int32>(!!0)
//! ldloca handler; call instance string ToStringAndClear()
//! ```
//!
//! Implementing those five methods natively is what makes `$"..."` work on
//! RustCLR. Without it every interpolated string in a program fails to resolve —
//! which, in modern C#, is most of them.
//!
//! The handler is a value type, so `this` arrives as a managed pointer into the
//! caller's local. The constructor writes an accumulator handle through that
//! pointer, and the append methods read it back — which works because managed
//! pointers in this runtime are structural and always resolve to the live slot.

use crate::support::*;
use rustclr_core::{ByRef, ClrString, ExecResult, ExecutionError, Interpreter, Value};
use rustclr_gc::Handle;

#[allow(unused_imports)]
use crate::prelude::*;

const TYPE: &str = "System.Runtime.CompilerServices.DefaultInterpolatedStringHandler";

pub fn register(interp: &mut Interpreter) {
    register_handler(interp, TYPE);
}

/// `StringBuilder.AppendInterpolatedStringHandler` has the same shape; its
/// constructor also takes the builder, which `StringBuilder.Append(ref
/// handler)` appends the accumulated text to. Full tier only.
pub(crate) fn register_builder_handler(interp: &mut Interpreter) {
    register_handler(interp, "System.Text.StringBuilder+AppendInterpolatedStringHandler");
    // (literalLength, formattedCount, builder, provider)
    interp.register_native("System.Text.StringBuilder+AppendInterpolatedStringHandler::.ctor/4", ctor);
}

/// The accumulated text of a handler passed by reference, cleared afterwards.
pub(crate) fn take_handler_text(interp: &mut Interpreter, handler: &Value) -> ExecResult<String> {
    let handle = match handler {
        Value::Ref(r) => interp.load_indirect_public(r.clone())?,
        other => other.clone(),
    }
    .as_handle()
    .filter(|h| !h.is_null())
    .ok_or_else(|| ExecutionError::InvalidProgram("the interpolated string handler was not constructed".into()))?;
    let text = interp
        .heap
        .with_mut::<ClrString, _>(handle, |s| {
            let t = String::from_utf16_lossy(&s.units);
            s.units.clear();
            t
        })
        .unwrap_or_default();
    Ok(text)
}

fn register_handler(interp: &mut Interpreter, ty: &'static str) {
    let key = |member: &str| -> &'static str { Box::leak(format!("{ty}::{member}").into_boxed_str()) };
    // Both constructor shapes: (literalLength, formattedCount) and the overload
    // that also takes an initial buffer capacity.
    interp.register_native(key(".ctor(int,int)"), ctor);
    interp.register_native(key(".ctor(int,int,#0)"), ctor);
    interp.register_native(key(".ctor/2"), ctor);
    interp.register_native(key(".ctor/3"), ctor);

    interp.register_native(key("AppendLiteral(string)"), append_literal);
    interp.register_native(key("AppendLiteral/1"), append_literal);

    // `AppendFormatted<T>` is generic. The loader gives each instantiation a
    // name carrying its concrete type argument, so `bool` can render as `True`
    // rather than as the `1` its erased int32 representation would print.
    for shape in [
        "bool", "char", "int", "uint", "long", "ulong", "short", "ushort",
        "sbyte", "byte", "float", "double", "string", "object",
    ] {
        interp.register_native(key(&format!("AppendFormatted({shape})")), append_formatted);
        interp.register_native(key(&format!("AppendFormatted({shape},int)")), append_formatted);
        interp.register_native(key(&format!("AppendFormatted({shape},string)")), append_formatted);
        interp.register_native(
            key(&format!("AppendFormatted({shape},int,string)")),
            append_formatted,
        );
    }
    // Anything else — a user type, an unresolved generic — falls back to arity.
    interp.register_native(key("AppendFormatted/1"), append_formatted);
    interp.register_native(key("AppendFormatted/2"), append_formatted);
    interp.register_native(key("AppendFormatted/3"), append_formatted);

    interp.register_native(key("ToStringAndClear()"), to_string_and_clear);
    interp.register_native(key("ToStringAndClear/0"), to_string_and_clear);
    interp.register_native(key("ToString()"), to_string);
    interp.register_native(key("Clear()"), clear);
}


/// Renders a value the way the interpolation hole should print it.
///
/// The erased representation cannot tell a `bool` from an `int` — both are
/// int32 on the evaluation stack — so the concrete type comes from the binding
/// key the loader chose for this instantiation.
fn render(interp: &mut Interpreter, value: &Value, as_bool: bool, as_char: bool) -> String {
    if as_bool {
        return if value.is_truthy() { "True".into() } else { "False".into() };
    }
    if as_char {
        let code = value.as_i32().unwrap_or(0);
        return char::from_u32(code as u32).map(String::from).unwrap_or_default();
    }
    display(interp, value)
}

/// `.ctor(int literalLength, int formattedCount)` — starts the accumulator.
///
/// The capacity hint is used, since the compiler already knows how long the
/// literal parts are; that saves a reallocation on almost every interpolation.
fn ctor(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let literal_length = args.get(1).and_then(|v| v.as_i32()).unwrap_or(0).max(0) as usize;
    let formatted_count = args.get(2).and_then(|v| v.as_i32()).unwrap_or(0).max(0) as usize;

    let accumulator = ClrString {
        units: Vec::with_capacity(literal_length + formatted_count * 8),
    };
    let handle = interp.alloc_clr_string(accumulator);

    store_handle(interp, args, handle)?;
    Ok(None)
}

fn append_literal(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let text = arg_string_or_empty(interp, args, 1)?;
    append(interp, args, &text)?;
    Ok(None)
}

fn append_formatted(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let value = arg(interp, args, 1)?;

    // Recover the concrete type argument from the binding key the loader used.
    let bound = interp
        .current_native_method()
        .map(|m| interp.loader.registry.method(m).qualified_name.clone())
        .unwrap_or_default();
    let type_hint = interp
        .current_native_method()
        .and_then(|m| interp.loader.registry.method(m).generic_args.first().copied());

    // (value), (value, alignment), (value, format), (value, alignment, format).
    let mut alignment = 0;
    let mut spec = String::new();
    for extra in args.iter().skip(2) {
        match extra {
            Value::I32(n) => alignment = *n,
            Value::Obj(h) => spec = interp.string_value(*h).unwrap_or_default(),
            _ => {}
        }
    }

    let text = if bound.contains("AppendFormatted(bool") {
        render(interp, &value, true, false)
    } else if bound.contains("AppendFormatted(char") {
        render(interp, &value, false, true)
    } else if type_hint.is_some() || !spec.is_empty() {
        // The type argument says what the slot holds: an enum prints its
        // name, a DateTime its date, and `{x:N2}` applies the specifier.
        crate::extras::format_value(interp, &value, type_hint, &spec)
    } else {
        render(interp, &value, false, false)
    };
    let padded = pad(&text, alignment);

    append(interp, args, &padded)?;
    Ok(None)
}

fn to_string_and_clear(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let handle = load_handle(interp, args)?;
    let units = interp
        .heap.with::<ClrString, _>(handle, |s| s.units.clone())
        .unwrap_or_default();

    // The result must be an independent string: the handler is cleared and its
    // storage reused by the next interpolation in the same method.
    let result = interp.alloc_clr_string(ClrString { units });
    interp.heap.with_mut::<ClrString, _>(handle, |accumulator| {
        accumulator.units.clear();
    });
    Ok(Some(Value::Obj(result)))
}

fn to_string(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let handle = load_handle(interp, args)?;
    let units = interp
        .heap.with::<ClrString, _>(handle, |s| s.units.clone())
        .unwrap_or_default();
    Ok(Some(Value::Obj(interp.alloc_clr_string(ClrString { units }))))
}

fn clear(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let handle = load_handle(interp, args)?;
    interp.heap.with_mut::<ClrString, _>(handle, |accumulator| {
        accumulator.units.clear();
    });
    Ok(None)
}

// -- accumulator plumbing ----------------------------------------------------

/// Writes the accumulator handle back through `this`.
fn store_handle(
    interp: &mut Interpreter,
    args: &[Value],
    handle: Handle,
) -> ExecResult<()> {
    match args.first() {
        Some(Value::Ref(target)) => {
            let target: ByRef = target.clone();
            interp.store_indirect_public(target, Value::Obj(handle))
        }
        // A handler passed by value cannot be written back to, which would make
        // every later append silently vanish. Say so instead.
        _ => Err(ExecutionError::InvalidProgram(
            "an interpolated string handler must be passed by reference".into(),
        )),
    }
}

/// Reads the accumulator handle out of `this`.
fn load_handle(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Handle> {
    let value = arg(interp, args, 0)?;
    value.as_handle().filter(|h| !h.is_null()).ok_or_else(|| {
        ExecutionError::InvalidProgram(
            "the interpolated string handler was used before it was constructed".into(),
        )
    })
}

fn append(interp: &mut Interpreter, args: &[Value], text: &str) -> ExecResult<()> {
    let handle = load_handle(interp, args)?;
    let addition: Vec<u16> = text.encode_utf16().collect();
    match interp
        .heap
        .with_mut::<ClrString, _>(handle, |accumulator| {
            accumulator.units.extend_from_slice(&addition)
        }) {
        Some(()) => Ok(()),
        None => Err(ExecutionError::InvalidProgram(
            "the interpolated string handler's buffer was collected".into(),
        )),
    }
}

/// Applies the alignment argument: positive right-justifies, negative left.
fn pad(text: &str, alignment: i32) -> String {
    if alignment == 0 {
        return text.to_string();
    }
    let width = alignment.unsigned_abs() as usize;
    let length = text.encode_utf16().count();
    if length >= width {
        return text.to_string();
    }
    let padding = " ".repeat(width - length);
    if alignment > 0 {
        format!("{padding}{text}")
    } else {
        format!("{text}{padding}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_pads_on_the_correct_side() {
        assert_eq!(pad("42", 5), "   42");
        assert_eq!(pad("42", -5), "42   ");
        assert_eq!(pad("42", 0), "42");
        assert_eq!(pad("longer", 3), "longer", "never truncates");
    }
}
