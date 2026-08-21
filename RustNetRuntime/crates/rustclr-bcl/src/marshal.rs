//! `System.Runtime.InteropServices.Marshal`, for blittable structs.
//!
//! Marshalling is turning a managed value into bytes and back. Both halves
//! became possible at once: a raw pointer here is a buffer plus a byte offset
//! (see [`rustclr_core::RawPtr`]), so there is somewhere to put the bytes, and
//! a generic method knows its type arguments, so `SizeOf<T>()` can ask what
//! `T` is rather than guess.
//!
//! # What is marshalled
//!
//! A **blittable** struct: one whose fields are primitives, laid out in
//! declaration order at their natural widths, which is what
//! `[StructLayout(LayoutKind.Sequential)]` means and what C# structs default
//! to. `Point { int X; int Y; }` is eight bytes with `Y` at offset four.
//!
//! Anything else is refused. A struct holding a reference has no byte image —
//! the field is a handle into the GC's table, and writing the handle's bits
//! would produce a number that looks like a pointer and is not one.
//!
//! # `AllocHGlobal` does not allocate unmanaged memory
//!
//! It allocates a byte buffer on the managed heap and hands back a pointer to
//! it. `FreeHGlobal` therefore does nothing: the collector owns the buffer and
//! reclaims it when the last pointer to it is gone. A program that frees
//! correctly sees no difference; one that uses memory after freeing it sees it
//! still valid here and a crash on .NET, which is the safe direction to differ
//! in. What a program cannot do is hand the pointer to native code — that
//! needs a real address, and `docs/limitations.md` says so.

use alloc::format;
use alloc::vec::Vec;

use rustclr_core::{
    ExecResult, ExecutionError, Interpreter, StructValue, TypeId, Value,
};

use crate::support::arg;

const MARSHAL: &str = "System.Runtime.InteropServices.Marshal";

pub fn register(interp: &mut Interpreter) {
    // `SizeOf<T>()` and `SizeOf(Type)`. The generic form is the interesting
    // one: `T` comes from the instantiation the call site recorded.
    interp.register_native(key("SizeOf()"), |i, _a| {
        let type_id = generic_argument(i)?;
        Ok(Some(Value::I32(i.size_of_public(type_id) as i32)))
    });
    interp.register_native(key("SizeOf/1"), |i, a| {
        // The non-generic overload takes a `Type`, or a boxed instance.
        let type_id = match arg(i, a, 0)?.as_handle().filter(|h| !h.is_null()) {
            Some(h) => i.type_from_object(h).or_else(|| i.type_of(h)),
            None => generic_argument(i).ok(),
        };
        let type_id = type_id.ok_or_else(|| {
            ExecutionError::exception(
                rustclr_core::ClrExceptionKind::Argument,
                "SizeOf was given something that does not name a type",
            )
        })?;
        Ok(Some(Value::I32(i.size_of_public(type_id) as i32)))
    });

    interp.register_native(key("AllocHGlobal/1"), |i, a| {
        let size = arg(i, a, 0)?.as_i64().unwrap_or(0).max(0) as usize;
        Ok(Some(Value::Ptr(i.alloc_pointer(size))))
    });
    // Nothing to free: the collector owns the buffer. See the module comment.
    interp.register_native(key("FreeHGlobal/1"), |_i, _a| Ok(None));
    interp.register_native(key("AllocCoTaskMem/1"), |i, a| {
        let size = arg(i, a, 0)?.as_i64().unwrap_or(0).max(0) as usize;
        Ok(Some(Value::Ptr(i.alloc_pointer(size))))
    });
    interp.register_native(key("FreeCoTaskMem/1"), |_i, _a| Ok(None));

    interp.register_native(key("StructureToPtr/3"), |i, a| {
        let value = arg(i, a, 0)?;
        let Value::Ptr(target) = arg(i, a, 1)? else {
            return Err(not_a_pointer("StructureToPtr"));
        };
        let (type_id, fields) = read_struct(i, &value)?;
        for (n, (offset, width)) in layout(i, type_id)?.into_iter().enumerate() {
            let field = fields.get(n).cloned().unwrap_or(Value::I32(0));
            i.write_pointer(target.offset_by(offset as i64), field, width)?;
        }
        Ok(None)
    });

    for member in ["PtrToStructure/1", "PtrToStructure/2"] {
        interp.register_native(key(member), |i, a| {
            let Value::Ptr(source) = arg(i, a, 0)? else {
                return Err(not_a_pointer("PtrToStructure"));
            };
            let type_id = generic_argument(i)?;
            let mut fields = Vec::new();
            for (offset, width) in layout(i, type_id)? {
                fields.push(i.read_pointer(source.offset_by(offset as i64), width)?);
            }
            Ok(Some(Value::Struct(alloc::boxed::Box::new(StructValue {
                type_id,
                fields,
            }))))
        });
    }
}

fn key(member: &str) -> &'static str {
    alloc::boxed::Box::leak(format!("{MARSHAL}::{member}").into_boxed_str())
}

fn not_a_pointer(member: &str) -> ExecutionError {
    ExecutionError::Unsupported(format!(
        "{member} on something that is not a pointer this runtime made; a pointer here is a \
         managed buffer plus an offset, not an address"
    ))
}

/// The single type argument of the generic method being serviced.
///
/// `Marshal.SizeOf<Point>()` records `Point` on the instantiation, so the
/// native implementation can read it back rather than being handed it.
fn generic_argument(interp: &Interpreter) -> ExecResult<TypeId> {
    let method = interp.current_native_method().ok_or_else(|| {
        ExecutionError::InvalidProgram("a marshalling call outside a native frame".into())
    })?;
    interp
        .loader
        .registry
        .method(method)
        .generic_args
        .first()
        .copied()
        .ok_or_else(|| {
            ExecutionError::Unsupported(
                "a marshalling call whose type argument this runtime could not recover".into(),
            )
        })
}

/// Reads a struct value, however it arrived.
fn read_struct(interp: &mut Interpreter, value: &Value) -> ExecResult<(TypeId, Vec<Value>)> {
    match value {
        Value::Struct(s) => Ok((s.type_id, s.fields.clone())),
        // Boxed, which is how it arrives when the parameter is `object`.
        Value::Obj(h) if !h.is_null() => {
            let boxed = interp
                .heap
                .with::<rustclr_core::ClrBox, _>(*h, |b| b.value.clone())
                .ok_or_else(|| {
                    ExecutionError::Unsupported("marshalling a reference type".into())
                })?;
            read_struct(interp, &boxed)
        }
        _ => Err(ExecutionError::Unsupported(
            "marshalling something that is not a struct".into(),
        )),
    }
}

/// Byte offset and width of each instance field, in declaration order.
///
/// Sequential layout at natural widths — what a C# struct gets by default and
/// what [`Interpreter::size_of_public`] already assumes, so the two agree by
/// construction rather than by coincidence.
fn layout(interp: &Interpreter, type_id: TypeId) -> ExecResult<Vec<(usize, usize)>> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for field in interp.instance_fields(type_id) {
        let info = interp.loader.registry.field(field);
        let width = blittable_width(&info.signature).ok_or_else(|| {
            ExecutionError::Unsupported(format!(
                "marshalling `{}`: its field `{}` is not blittable, so it has no byte image",
                interp.loader.registry.ty(type_id).full_name(),
                info.name
            ))
        })?;
        // An explicit `[FieldOffset]` wins where one is given.
        let at = info.offset.map(|o| o as usize).unwrap_or(offset);
        out.push((at, width));
        offset = at + width;
    }
    Ok(out)
}

/// Bytes a field occupies, or `None` when it has no byte image at all.
fn blittable_width(sig: &rustclr_core::metadata::TypeSig) -> Option<usize> {
    use rustclr_core::metadata::TypeSig;
    Some(match sig.unwrap_modifiers() {
        TypeSig::I1 | TypeSig::U1 | TypeSig::Boolean => 1,
        TypeSig::I2 | TypeSig::U2 | TypeSig::Char => 2,
        TypeSig::I4 | TypeSig::U4 | TypeSig::R4 => 4,
        TypeSig::I8 | TypeSig::U8 | TypeSig::R8 => 8,
        // A reference field is a handle into the GC's table. Writing its bits
        // would produce a number that looks like a pointer and is not one.
        _ => return None,
    })
}
