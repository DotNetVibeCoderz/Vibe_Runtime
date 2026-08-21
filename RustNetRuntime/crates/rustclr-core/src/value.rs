//! Evaluation-stack values.
//!
//! ECMA-335 III.1.1 defines a deliberately small set of stack types: `int32`,
//! `int64`, `native int`, `F`, `O` (object reference), `&` (managed pointer)
//! and user-defined value types. Everything narrower — `bool`, `char`, `int8`,
//! `int16` and their unsigned forms — is widened to `int32` on load and
//! truncated on store, which is why there is no `Value::I8` here.

use crate::types::TypeId;
use rustclr_gc::Handle;

#[allow(unused_imports)]
use crate::prelude::*;

/// Where a managed pointer (`&`) points.
///
/// Interior pointers are represented structurally rather than as raw addresses.
/// A raw pointer into a GC heap would have to be updated by the collector and
/// could dangle; this form is always safe to resolve and always current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ByRef {
    /// A local variable of the frame identified by `frame`.
    Local { frame: u32, index: u32 },
    /// An argument of the frame identified by `frame`.
    Arg { frame: u32, index: u32 },
    /// An instance field of a heap object.
    Field { object: Handle, slot: u32 },
    /// A static field of a type.
    Static { type_id: TypeId, slot: u32 },
    /// An element of a single-dimension array.
    ArrayElement { array: Handle, index: u32 },
    /// A field of a value-type instance that is itself addressed by `base`.
    ///
    /// `p.X` where `p` is a struct in a local has no heap object to point at,
    /// so the pointer is a *path*: the pointer to `p`, plus the field slot.
    /// Composing rather than flattening is what lets `a.b.c` work — and it is
    /// the reason `ByRef` is not `Copy`.
    StructField { base: Box<ByRef>, slot: u32 },
}

/// One evaluation-stack slot.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// The untyped null reference.
    Null,
    /// `int32`, and every narrower integer widened to it.
    I32(i32),
    /// `int64`.
    I64(i64),
    /// `native int`, the width of a pointer on the target.
    NativeInt(i64),
    /// `F`. The CLR has a single floating stack type; `conv.r4` rounds through
    /// `f32` but the slot stays 64-bit.
    F(f64),
    /// `O`: a reference to a heap object.
    Obj(Handle),
    /// `&`: a managed pointer.
    Ref(ByRef),
    /// An unboxed value-type instance living directly in the slot.
    Struct(Box<StructValue>),
    /// A method pointer produced by `ldftn` / `ldvirtftn`.
    FnPtr(crate::types::MethodId),
    /// An unmanaged pointer: a byte offset into a buffer on the managed heap.
    ///
    /// Not an address. C# reaches for one through `stackalloc` or `fixed`, and
    /// both name memory this runtime already owns — `localloc` allocates a byte
    /// array, and `fixed` pins an existing one — so a pointer can be the pair
    /// (buffer, byte offset) and stay inside the handle table. Arithmetic moves
    /// the offset; nothing here can be made to point at memory the runtime does
    /// not own, which is the property that keeps `unsafe` C# safe underneath.
    ///
    /// This is why [`ByRef`] and this are different things. A `ByRef` is a
    /// *path* to a slot — a local, a field, an element — and cannot be
    /// byte-addressed or advanced past its target. `conv.u` on one produces
    /// this, which is exactly what `fixed` compiles to.
    Ptr(RawPtr),
}

/// An unmanaged pointer: a buffer and a byte offset into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawPtr {
    /// The array the pointer is into. Null for a null pointer.
    pub buffer: Handle,
    /// Bytes from the start of the buffer. Signed, because pointer arithmetic
    /// may legally step outside a buffer as long as nothing dereferences there.
    pub offset: i64,
}

impl RawPtr {
    pub const NULL: RawPtr = RawPtr { buffer: Handle::NULL, offset: 0 };

    pub fn is_null(&self) -> bool {
        self.buffer.is_null()
    }

    pub fn offset_by(self, bytes: i64) -> RawPtr {
        RawPtr { buffer: self.buffer, offset: self.offset + bytes }
    }
}

/// The contents of an unboxed value type.
#[derive(Debug, Clone, PartialEq)]
pub struct StructValue {
    pub type_id: TypeId,
    pub fields: Vec<Value>,
}

impl Value {
    /// The zero value for a freshly initialised slot of unknown type.
    pub const ZERO: Value = Value::I32(0);

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null) || matches!(self, Value::Obj(h) if h.is_null())
    }

    /// Reads the slot as `int32`, widening `native int` where the CLR allows it.
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::I32(v) => Some(*v),
            Value::NativeInt(v) => Some(*v as i32),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::I64(v) => Some(*v),
            Value::I32(v) => Some(*v as i64),
            Value::NativeInt(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::F(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_handle(&self) -> Option<Handle> {
        match self {
            Value::Obj(h) => Some(*h),
            Value::Null => Some(Handle::NULL),
            _ => None,
        }
    }

    pub fn as_byref(&self) -> Option<ByRef> {
        match self {
            Value::Ref(r) => Some(r.clone()),
            _ => None,
        }
    }

    /// Truthiness for `brtrue` / `brfalse`: zero, null and NaN-free zero float
    /// are false.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::I32(v) => *v != 0,
            Value::I64(v) => *v != 0,
            Value::NativeInt(v) => *v != 0,
            Value::F(v) => *v != 0.0,
            Value::Obj(h) => !h.is_null(),
            Value::Ref(_) | Value::FnPtr(_) => true,
            Value::Struct(_) => true,
            // A null pointer is false, exactly as `brfalse` on one expects.
            Value::Ptr(p) => !p.is_null() || p.offset != 0,
        }
    }

    /// The stack-type name used in verification errors.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::I32(_) => "int32",
            Value::I64(_) => "int64",
            Value::NativeInt(_) => "native int",
            Value::F(_) => "F",
            Value::Obj(_) => "O",
            Value::Ref(_) => "&",
            Value::Struct(_) => "value type",
            Value::FnPtr(_) => "method pointer",
            Value::Ptr(_) => "native int",
        }
    }

    /// Object handles reachable from this slot, for GC root scanning.
    pub fn trace_handles(&self, out: &mut Vec<Handle>) {
        match self {
            Value::Obj(h) if !h.is_null() => out.push(*h),
            Value::Ref(ByRef::Field { object, .. }) => out.push(*object),
            Value::Ref(ByRef::ArrayElement { array, .. }) => out.push(*array),
            // A raw pointer roots the buffer it points into. `stackalloc`
            // memory has no other reference anywhere, so without this a
            // collection between the `localloc` and the next write frees it.
            Value::Ptr(p) if !p.is_null() => out.push(p.buffer),
            // A pointer into a struct roots whatever its base roots: the
            // struct may live in a field of a heap object that nothing else
            // on the stack refers to.
            Value::Ref(ByRef::StructField { base, .. }) => {
                Value::Ref((**base).clone()).trace_handles(out)
            }
            Value::Struct(s) => {
                for f in &s.fields {
                    f.trace_handles(out);
                }
            }
            _ => {}
        }
    }
}

impl core::fmt::Display for Value {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::I32(v) => write!(f, "{v}"),
            Value::I64(v) => write!(f, "{v}"),
            Value::NativeInt(v) => write!(f, "{v}n"),
            Value::F(v) => write!(f, "{v}"),
            Value::Obj(h) => write!(f, "{h:?}"),
            Value::Ref(r) => write!(f, "&{r:?}"),
            Value::Struct(s) => write!(f, "struct#{}", s.type_id.0),
            Value::FnPtr(m) => write!(f, "fn#{}", m.0),
            Value::Ptr(p) => write!(f, "{:?}+{}", p.buffer, p.offset),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_integers_are_represented_as_int32() {
        assert_eq!(Value::I32(-1).as_i32(), Some(-1));
        assert_eq!(Value::I32(1).as_i64(), Some(1));
    }

    #[test]
    fn truthiness_matches_brtrue_semantics() {
        assert!(!Value::I32(0).is_truthy());
        assert!(Value::I32(-1).is_truthy());
        assert!(!Value::Null.is_truthy());
        assert!(!Value::Obj(Handle::NULL).is_truthy());
        assert!(!Value::F(0.0).is_truthy());
    }

    #[test]
    fn tracing_finds_handles_nested_in_structs() {
        let inner = Value::Struct(Box::new(StructValue {
            type_id: TypeId(1),
            fields: vec![Value::I32(3), Value::Obj(Handle::from_bits(0x1_0000_0001))],
        }));
        let mut out = Vec::new();
        inner.trace_handles(&mut out);
        assert_eq!(out.len(), 1);
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    /// Every evaluation-stack slot, every local and every argument is a
    /// `Value`, so its width is multiplied by everything the interpreter does.
    /// It is 24 bytes because `ByRef` is 16 and needs a discriminant beside it.
    ///
    /// This is here because adding `Value::Ptr` looked like it should have cost
    /// something and did not: `RawPtr` is a handle and an offset, 16 bytes, so
    /// it fits in the room `ByRef` had already taken. A future variant that is
    /// wider would widen every slot in the runtime, which is worth failing a
    /// test over rather than discovering in a benchmark.
    #[test]
    fn a_value_stays_three_words() {
        assert_eq!(core::mem::size_of::<ByRef>(), 16);
        assert_eq!(core::mem::size_of::<RawPtr>(), 16);
        assert_eq!(core::mem::size_of::<Value>(), 24);
    }
}
