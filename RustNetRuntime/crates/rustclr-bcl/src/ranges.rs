//! `System.Index`, `System.Range` and array slicing.
//!
//! `a[^1]` and `a[1..4]` are C# 8 features that survive to run time as real
//! types. The compiler builds an `Index` (an int where a from-end index is
//! stored as its bitwise complement), pairs two of them into a `Range`, and
//! calls `RuntimeHelpers.GetSubArray` to produce the slice.
//!
//! Both are value types, so `this` arrives as a managed pointer and the
//! constructors write through it — the same mechanism the interpolated string
//! handler uses.

use crate::support::*;
use rustclr_core::{
    ByRef, ClrArray, ExecResult, ExecutionError, Interpreter, StructValue, Value,
};

pub fn register(interp: &mut Interpreter) {
    register_index(interp);
    register_range(interp);
    register_tuples(interp);
    register_nullable(interp);

    // The element type comes from the source array, so one implementation
    // covers every instantiation; the arity key is what binds.
    interp.register_native(
        "System.Runtime.CompilerServices.RuntimeHelpers::GetSubArray/2",
        get_sub_array,
    );
}

// ── System.Index ─────────────────────────────────────────────────────────────

/// .NET stores a from-end index as `~value`, which keeps `Index` a single int.
fn encode(value: i32, from_end: bool) -> i32 {
    if from_end { !value } else { value }
}

fn register_index(interp: &mut Interpreter) {
    interp.register_native("System.Index::.ctor(int,bool)", |i, a| {
        let value = arg_i32(i, a, 1)?;
        let from_end = arg_bool(i, a, 2)?;
        write_through(i, a, Value::I32(encode(value, from_end)))
    });
    interp.register_native("System.Index::.ctor(int)", |i, a| {
        let value = arg_i32(i, a, 1)?;
        write_through(i, a, Value::I32(value))
    });

    // `Index start = 2;` goes through the implicit conversion.
    interp.register_native("System.Index::op_Implicit(int)", |i, a| {
        Ok(Some(Value::I32(arg_i32(i, a, 0)?)))
    });
    interp.register_native("System.Index::FromStart(int)", |i, a| {
        Ok(Some(Value::I32(arg_i32(i, a, 0)?)))
    });
    interp.register_native("System.Index::FromEnd(int)", |i, a| {
        Ok(Some(Value::I32(encode(arg_i32(i, a, 0)?, true))))
    });

    interp.register_native("System.Index::get_Value()", |i, a| {
        let encoded = arg_i32(i, a, 0)?;
        Ok(Some(Value::I32(if encoded < 0 { !encoded } else { encoded })))
    });
    interp.register_native("System.Index::get_IsFromEnd()", |i, a| {
        Ok(Some(Value::I32((arg_i32(i, a, 0)? < 0) as i32)))
    });
    interp.register_native("System.Index::GetOffset(int)", |i, a| {
        let encoded = arg_i32(i, a, 0)?;
        let length = arg_i32(i, a, 1)?;
        Ok(Some(Value::I32(offset_of(encoded, length))))
    });
}

/// Resolves an encoded index against a collection length.
fn offset_of(encoded: i32, length: i32) -> i32 {
    if encoded < 0 {
        // `~encoded` recovers the from-end distance.
        length - !encoded
    } else {
        encoded
    }
}

// ── System.Range ─────────────────────────────────────────────────────────────

fn register_range(interp: &mut Interpreter) {
    interp.register_native("System.Range::.ctor(#0,#0)", range_ctor);
    interp.register_native("System.Range::.ctor/2", range_ctor);

    interp.register_native("System.Range::get_Start()", |i, a| {
        Ok(Some(Value::I32(range_bounds(i, a, 0)?)))
    });
    interp.register_native("System.Range::get_End()", |i, a| {
        Ok(Some(Value::I32(range_bounds(i, a, 1)?)))
    });

    interp.register_native("System.Range::StartAt(#0)", |i, a| {
        let start = arg_i32(i, a, 0)?;
        Ok(Some(make_range(i, start, encode(0, true))))
    });
    interp.register_native("System.Range::EndAt(#0)", |i, a| {
        let end = arg_i32(i, a, 0)?;
        Ok(Some(make_range(i, 0, end)))
    });
    interp.register_native("System.Range::get_All()", |i, _a| {
        Ok(Some(make_range(i, 0, encode(0, true))))
    });
}

fn range_ctor(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let start = arg_i32(interp, args, 1)?;
    let end = arg_i32(interp, args, 2)?;
    let range = make_range(interp, start, end);
    write_through(interp, args, range)
}

/// A `Range` is carried as a two-field struct value: the encoded bounds.
fn make_range(interp: &Interpreter, start: i32, end: i32) -> Value {
    let type_id = interp
        .loader
        .registry
        .find_type_by_name("System.Range")
        .unwrap_or(interp.loader.core().object);

    Value::Struct(Box::new(StructValue {
        type_id,
        fields: vec![Value::I32(start), Value::I32(end)],
    }))
}

fn range_bounds(interp: &mut Interpreter, args: &[Value], field: usize) -> ExecResult<i32> {
    match arg(interp, args, 0)? {
        Value::Struct(s) => Ok(s.fields.get(field).and_then(|v| v.as_i32()).unwrap_or(0)),
        other => Ok(other.as_i32().unwrap_or(0)),
    }
}

// ── Tuples ───────────────────────────────────────────────────────────────────

/// `ValueTuple`N` constructors.
///
/// The elements are stored as a struct value whose field order matches the
/// `Item1…ItemN` slots the loader registered, so `ldfld Item2` resolves without
/// any special case in the interpreter.
fn register_tuples(interp: &mut Interpreter) {
    for arity in 1..=8usize {
        let type_name = format!("System.ValueTuple`{arity}");
        // `this` is a managed pointer, so the constructor takes arity + 1 args.
        let key: &'static str =
            Box::leak(format!("{type_name}::.ctor/{arity}").into_boxed_str());
        interp.register_native(key, tuple_ctor);
    }
}

fn tuple_ctor(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let type_id = interp
        .current_native_method()
        .map(|m| interp.loader.registry.method(m).declaring_type)
        .unwrap_or(interp.loader.core().object);

    let mut fields = Vec::with_capacity(args.len().saturating_sub(1));
    for index in 1..args.len() {
        fields.push(arg(interp, args, index)?);
    }

    let tuple = Value::Struct(Box::new(StructValue { type_id, fields }));
    write_through(interp, args, tuple)
}

// ── Nullable ─────────────────────────────────────────────────────────────────

/// `System.Nullable<T>`, carried as a two-field struct: a flag and a payload.
///
/// `initobj` zeroes both, which is exactly "no value" — so the default case
/// needs no special handling.
fn register_nullable(interp: &mut Interpreter) {
    const T: &str = "System.Nullable`1";

    interp.register_native(leak(format!("{T}::.ctor/1")), |i, a| {
        let payload = arg(i, a, 1)?;
        let value = nullable(i, true, payload);
        write_through(i, a, value)
    });

    interp.register_native(leak(format!("{T}::get_HasValue()")), |i, a| {
        Ok(Some(Value::I32(has_value(i, a)? as i32)))
    });

    interp.register_native(leak(format!("{T}::get_Value()")), |i, a| {
        if !has_value(i, a)? {
            return Err(ExecutionError::exception(
                rustclr_core::ClrExceptionKind::InvalidOperation,
                "Nullable object must have a value.",
            ));
        }
        payload(i, a)
    });

    interp.register_native(leak(format!("{T}::GetValueOrDefault()")), |i, a| {
        if has_value(i, a)? { payload(i, a) } else { Ok(Some(Value::I32(0))) }
    });
    interp.register_native(leak(format!("{T}::GetValueOrDefault/1")), |i, a| {
        if has_value(i, a)? { payload(i, a) } else { Ok(Some(arg(i, a, 1)?)) }
    });
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn nullable(interp: &Interpreter, has: bool, payload: Value) -> Value {
    let type_id = interp
        .loader
        .registry
        .find_type_by_name("System.Nullable`1")
        .unwrap_or(interp.loader.core().object);
    Value::Struct(Box::new(StructValue {
        type_id,
        fields: vec![Value::I32(has as i32), payload],
    }))
}

fn has_value(interp: &mut Interpreter, args: &[Value]) -> ExecResult<bool> {
    match arg(interp, args, 0)? {
        Value::Struct(s) => Ok(s.fields.first().is_some_and(|v| v.is_truthy())),
        // A zeroed local that never became a struct is "no value".
        _ => Ok(false),
    }
}

fn payload(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    match arg(interp, args, 0)? {
        Value::Struct(s) => Ok(Some(s.fields.get(1).cloned().unwrap_or(Value::I32(0)))),
        _ => Ok(Some(Value::I32(0))),
    }
}

// ── Slicing ──────────────────────────────────────────────────────────────────

/// `RuntimeHelpers.GetSubArray<T>(T[] array, Range range)`.
fn get_sub_array(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let source = arg_handle(interp, args, 0)?;
    if source.is_null() {
        return Err(ExecutionError::null_reference());
    }

    let (element_type, length) = match interp.heap.get_as::<ClrArray>(source) {
        Some(array) => (array.element_type, array.len() as i32),
        None => return Err(ExecutionError::null_reference()),
    };

    let bounds = arg(interp, args, 1)?;
    let (start_encoded, end_encoded) = match &bounds {
        Value::Struct(s) => (
            s.fields.first().and_then(|v| v.as_i32()).unwrap_or(0),
            s.fields.get(1).and_then(|v| v.as_i32()).unwrap_or(encode(0, true)),
        ),
        // A bare int means the whole array; the compiler should not emit this,
        // but degrading to a copy beats throwing.
        _ => (0, encode(0, true)),
    };

    let start = offset_of(start_encoded, length);
    let end = offset_of(end_encoded, length);

    if start < 0 || end > length || start > end {
        return Err(out_of_range("range"));
    }

    let count = (end - start) as usize;
    let slice = interp.alloc_array(element_type, count);

    // Copy element by element through the storage abstraction, so a primitive
    // source stays primitive in the slice.
    for index in 0..count {
        let value = interp
            .heap
            .get_as::<ClrArray>(source)
            .and_then(|a| a.storage.get(start as usize + index));
        if let (Some(value), Some(target)) = (value, interp.heap.get_as_mut::<ClrArray>(slice)) {
            target.storage.set(index, &value);
        }
    }

    Ok(Some(Value::Obj(slice)))
}

/// Completes a value-type constructor.
///
/// There are two call shapes and both are legal. `ldloca; call .ctor` passes a
/// managed pointer to an existing slot, so the value is written through it.
/// `newobj` has no slot yet, so the value is returned and the caller pushes it.
fn write_through(
    interp: &mut Interpreter,
    args: &[Value],
    value: Value,
) -> ExecResult<Option<Value>> {
    match args.first() {
        Some(Value::Ref(target)) => {
            let target: ByRef = *target;
            interp.store_indirect_public(target, value)?;
            Ok(None)
        }
        _ => Ok(Some(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_end_indices_round_trip() {
        let one_from_end = encode(1, true);
        assert!(one_from_end < 0, "a from-end index is stored complemented");
        assert_eq!(offset_of(one_from_end, 5), 4, "^1 of a 5-element array is index 4");
        assert_eq!(offset_of(encode(0, true), 5), 5, "^0 is the end");
        assert_eq!(offset_of(2, 5), 2, "a from-start index is itself");
    }
}
