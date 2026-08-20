//! Shared helpers for native BCL methods.

use rustclr_core::{
    ClrArray, ClrBox, ClrException, ClrObject, ClrString, ExecResult, ExecutionError, Interpreter,
    Primitive, TypeKind, Value,
};
use rustclr_gc::Handle;

/// Reads argument `i`, dereferencing a managed pointer if one was passed.
///
/// C# calls instance methods on value types through `ldloca`, so `this` for
/// `Int32.ToString()` arrives as a `&`, not a value.
pub fn arg(interp: &mut Interpreter, args: &[Value], i: usize) -> ExecResult<Value> {
    let Some(v) = args.get(i) else {
        return Ok(Value::Null);
    };
    match v {
        Value::Ref(r) => interp.load_indirect_public(*r),
        other => Ok(other.clone()),
    }
}

/// Reads argument `i` as a string, treating `null` as an error.
pub fn arg_string(interp: &mut Interpreter, args: &[Value], i: usize) -> ExecResult<String> {
    let v = arg(interp, args, i)?;
    match &v {
        Value::Obj(h) => interp
            .string_value(*h)
            .ok_or_else(|| ExecutionError::exception(
                rustclr_core::ClrExceptionKind::ArgumentNull,
                "Value cannot be null.",
            )),
        Value::Null => Err(ExecutionError::exception(
            rustclr_core::ClrExceptionKind::ArgumentNull,
            "Value cannot be null.",
        )),
        _ => Ok(display(interp, &v)),
    }
}

/// Reads argument `i` as a string, mapping `null` to the empty string.
pub fn arg_string_or_empty(interp: &mut Interpreter, args: &[Value], i: usize) -> ExecResult<String> {
    let v = arg(interp, args, i)?;
    Ok(match &v {
        Value::Obj(h) => interp.string_value(*h).unwrap_or_default(),
        _ => String::new(),
    })
}

pub fn arg_i32(interp: &mut Interpreter, args: &[Value], i: usize) -> ExecResult<i32> {
    Ok(arg(interp, args, i)?.as_i32().unwrap_or(0))
}

pub fn arg_i64(interp: &mut Interpreter, args: &[Value], i: usize) -> ExecResult<i64> {
    Ok(arg(interp, args, i)?.as_i64().unwrap_or(0))
}

pub fn arg_f64(interp: &mut Interpreter, args: &[Value], i: usize) -> ExecResult<f64> {
    let v = arg(interp, args, i)?;
    Ok(match v {
        Value::F(f) => f,
        other => other.as_i64().unwrap_or(0) as f64,
    })
}

pub fn arg_bool(interp: &mut Interpreter, args: &[Value], i: usize) -> ExecResult<bool> {
    Ok(arg(interp, args, i)?.is_truthy())
}

pub fn arg_handle(interp: &mut Interpreter, args: &[Value], i: usize) -> ExecResult<Handle> {
    Ok(arg(interp, args, i)?.as_handle().unwrap_or(Handle::NULL))
}

/// The line terminator `Console.WriteLine` and `Environment.NewLine` emit.
///
/// .NET uses CRLF on Windows and LF elsewhere. Writing a bare LF everywhere
/// would make RustCLR's output differ from the reference runtime on Windows —
/// which a byte-for-byte diff of the two catches immediately.
pub const NEWLINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// Wraps a Rust string as a managed string value.
pub fn string_value(interp: &mut Interpreter, s: &str) -> Value {
    Value::Obj(interp.alloc_string(s))
}

/// Renders a value the way `Object.ToString` would.
///
/// For a managed object with an overridden `ToString`, this calls it; the
/// result is what `Console.WriteLine(object)` and string concatenation need.
pub fn display(interp: &mut Interpreter, v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::I32(i) => i.to_string(),
        Value::I64(i) => i.to_string(),
        Value::NativeInt(i) => i.to_string(),
        Value::F(f) => format_double(*f),
        Value::FnPtr(_) => "System.IntPtr".into(),
        Value::Ref(r) => match interp.load_indirect_public(*r) {
            Ok(inner) => display(interp, &inner),
            Err(_) => String::new(),
        },
        Value::Struct(s) => interp.loader.registry.ty(s.type_id).full_name(),
        Value::Obj(h) => display_handle(interp, *h),
    }
}

fn display_handle(interp: &mut Interpreter, h: Handle) -> String {
    if h.is_null() {
        return String::new();
    }
    if let Some(s) = interp.string_value(h) {
        return s;
    }
    // A boxed primitive prints as its payload.
    if let Some(b) = interp.heap.get_as::<ClrBox>(h) {
        let inner = b.value.clone();
        let type_id = b.type_id;
        if interp.loader.registry.ty(type_id).kind == TypeKind::Enum {
            return inner.as_i64().unwrap_or(0).to_string();
        }
        return display(interp, &inner);
    }
    if let Some(e) = interp.heap.get_as::<ClrException>(h) {
        let name = interp.loader.registry.ty(e.type_id).full_name();
        let message = e.message.clone();
        return if message.is_empty() { name } else { format!("{name}: {message}") };
    }
    if let Some(a) = interp.heap.get_as::<ClrArray>(h) {
        let element = interp.loader.registry.ty(a.element_type).full_name();
        return format!("{element}[]");
    }

    // A managed object may override ToString; call it when it does.
    if let Some(type_id) = interp.type_of(h) {
        if let Some(method) = find_to_string(interp, type_id) {
            if let Ok(Some(Value::Obj(result))) = interp.invoke(method, vec![Value::Obj(h)]) {
                if let Some(s) = interp.string_value(result) {
                    return s;
                }
            }
        }
        return interp.loader.registry.ty(type_id).full_name();
    }
    "System.Object".into()
}

/// Finds a user-declared `ToString()` override, if any.
fn find_to_string(interp: &Interpreter, type_id: rustclr_core::TypeId) -> Option<rustclr_core::MethodId> {
    for t in interp.loader.registry.base_chain(type_id) {
        for m in &interp.loader.registry.ty(t).methods {
            let info = interp.loader.registry.method(*m);
            if info.name == "ToString"
                && info.signature.params.is_empty()
                && matches!(info.kind, rustclr_core::MethodKind::Il(_))
            {
                return Some(*m);
            }
        }
    }
    None
}

/// Formats a double the way .NET's default `ToString()` does: shortest
/// round-trippable form, with no trailing `.0` for integral values.
pub fn format_double(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "∞".into() } else { "-∞".into() };
    }
    if f == f.trunc() && f.abs() < 1e15 {
        return format!("{}", f as i64);
    }
    let s = format!("{f}");
    s
}

/// Formats a float, which .NET renders with single precision.
pub fn format_single(f: f32) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "∞".into() } else { "-∞".into() };
    }
    if f == f.trunc() && f.abs() < 1e7 {
        return format!("{}", f as i64);
    }
    format!("{f}")
}

/// Allocates a managed `char[]` from a string.
pub fn char_array(interp: &mut Interpreter, s: &str) -> Handle {
    let char_type = interp.loader.primitive_type(Primitive::Char);
    let units: Vec<u16> = s.encode_utf16().collect();
    let array = interp.alloc_array(char_type, units.len());
    if let Some(a) = interp.heap.get_as_mut::<ClrArray>(array) {
        for (i, u) in units.iter().enumerate() {
            a.storage.set(i, &Value::I32(*u as i32));
        }
    }
    array
}

/// Allocates a managed `string[]`.
pub fn string_array(interp: &mut Interpreter, items: &[String]) -> Handle {
    let string_type = interp.loader.core().string;
    let array = interp.alloc_array(string_type, items.len());
    for (i, s) in items.iter().enumerate() {
        let h = interp.alloc_string(s);
        if let Some(a) = interp.heap.get_as_mut::<ClrArray>(array) {
            a.storage.set(i, &Value::Obj(h));
        }
    }
    array
}

/// Reads a managed array into a vector of values.
pub fn array_values(interp: &Interpreter, h: Handle) -> Vec<Value> {
    match interp.heap.get_as::<ClrArray>(h) {
        Some(a) => (0..a.len()).filter_map(|i| a.storage.get(i)).collect(),
        None => Vec::new(),
    }
}

/// Throws `ArgumentOutOfRangeException`.
pub fn out_of_range(param: &str) -> ExecutionError {
    ExecutionError::exception(
        rustclr_core::ClrExceptionKind::ArgumentOutOfRange,
        format!("Specified argument was out of the range of valid values. (Parameter '{param}')"),
    )
}

/// Throws `FormatException`.
pub fn bad_format(what: &str) -> ExecutionError {
    ExecutionError::exception(
        rustclr_core::ClrExceptionKind::Format,
        format!("The input string '{what}' was not in a correct format."),
    )
}

/// Reads the field values of a plain managed object, for diagnostics.
pub fn object_fields(interp: &Interpreter, h: Handle) -> Vec<Value> {
    interp
        .heap
        .get_as::<ClrObject>(h)
        .map(|o| o.fields.clone())
        .unwrap_or_default()
}

/// Reads a managed string handle, or the empty string when it is not one.
pub fn read_string(interp: &Interpreter, h: Handle) -> String {
    interp
        .heap
        .get_as::<ClrString>(h)
        .map(|s| s.to_rust_string())
        .unwrap_or_default()
}
