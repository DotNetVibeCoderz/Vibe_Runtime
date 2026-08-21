//! Heap object representations.

use crate::types::{MethodId, Primitive, TypeId};
use crate::value::Value;
use rustclr_gc::{GcObject, Handle, Tracer};
use core::any::Any;

#[allow(unused_imports)]
use crate::prelude::*;

/// An instance of a reference type: a type id plus its instance-field slots.
#[derive(Debug)]
pub struct ClrObject {
    pub type_id: TypeId,
    pub fields: Vec<Value>,
    /// Monitor state, for `lock` / `Monitor.Enter`.
    pub monitor: Option<Box<MonitorState>>,
}

#[derive(Debug, Default)]
pub struct MonitorState {
    pub owner_thread: u64,
    pub recursion: u32,
}

impl ClrObject {
    pub fn new(type_id: TypeId, field_count: usize) -> Self {
        Self {
            type_id,
            fields: vec![Value::Null; field_count],
            monitor: None,
        }
    }
}

impl GcObject for ClrObject {
    fn trace(&self, tracer: &mut Tracer) {
        let mut handles = Vec::new();
        for f in &self.fields {
            f.trace_handles(&mut handles);
        }
        tracer.edges(handles);
    }

    fn size_hint(&self) -> usize {
        32 + self.fields.len() * core::mem::size_of::<Value>()
    }

    fn type_name(&self) -> &str {
        "object"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Backing storage for an array.
///
/// Arrays of primitives are stored unboxed. A `byte[1_000_000]` costs a
/// megabyte here rather than the 24 MB a `Vec<Value>` would need, which matters
/// on the memory-constrained targets this runtime supports.
#[derive(Debug)]
pub enum ArrayStorage {
    Bool(Vec<bool>),
    I8(Vec<i8>),
    U8(Vec<u8>),
    I16(Vec<i16>),
    U16(Vec<u16>),
    Char(Vec<u16>),
    I32(Vec<i32>),
    U32(Vec<u32>),
    I64(Vec<i64>),
    U64(Vec<u64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
    /// Reference elements.
    Refs(Vec<Handle>),
    /// Value-type elements, or anything without a specialised representation.
    Values(Vec<Value>),
}

impl ArrayStorage {
    /// Bytes per element, as the CLR lays the array out.
    ///
    /// This is what turns a raw pointer's byte offset back into an index:
    /// `fixed (int* p = values)` gives a pointer into `values`, `p + 1` moves
    /// it four bytes, and reading through it has to land on element one.
    /// `None` for storage whose elements are not a fixed width of bytes —
    /// references and boxed values — where byte addressing has no meaning.
    pub fn element_width(&self) -> Option<usize> {
        Some(match self {
            ArrayStorage::Bool(_) | ArrayStorage::I8(_) | ArrayStorage::U8(_) => 1,
            ArrayStorage::I16(_) | ArrayStorage::U16(_) | ArrayStorage::Char(_) => 2,
            ArrayStorage::I32(_) | ArrayStorage::U32(_) | ArrayStorage::F32(_) => 4,
            ArrayStorage::I64(_) | ArrayStorage::U64(_) | ArrayStorage::F64(_) => 8,
            ArrayStorage::Refs(_) | ArrayStorage::Values(_) => return None,
        })
    }

    /// Allocates zero-initialised storage of `len` elements.
    pub fn zeroed(primitive: Option<Primitive>, is_reference: bool, len: usize) -> Self {
        match primitive {
            Some(Primitive::Boolean) => Self::Bool(vec![false; len]),
            Some(Primitive::SByte) => Self::I8(vec![0; len]),
            Some(Primitive::Byte) => Self::U8(vec![0; len]),
            Some(Primitive::Int16) => Self::I16(vec![0; len]),
            Some(Primitive::UInt16) => Self::U16(vec![0; len]),
            Some(Primitive::Char) => Self::Char(vec![0; len]),
            Some(Primitive::Int32) => Self::I32(vec![0; len]),
            Some(Primitive::UInt32) => Self::U32(vec![0; len]),
            Some(Primitive::Int64) | Some(Primitive::IntPtr) => Self::I64(vec![0; len]),
            Some(Primitive::UInt64) | Some(Primitive::UIntPtr) => Self::U64(vec![0; len]),
            Some(Primitive::Single) => Self::F32(vec![0.0; len]),
            Some(Primitive::Double) => Self::F64(vec![0.0; len]),
            _ if is_reference => Self::Refs(vec![Handle::NULL; len]),
            _ => Self::Values(vec![Value::I32(0); len]),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Bool(v) => v.len(),
            Self::I8(v) => v.len(),
            Self::U8(v) => v.len(),
            Self::I16(v) => v.len(),
            Self::U16(v) | Self::Char(v) => v.len(),
            Self::I32(v) => v.len(),
            Self::U32(v) => v.len(),
            Self::I64(v) => v.len(),
            Self::U64(v) => v.len(),
            Self::F32(v) => v.len(),
            Self::F64(v) => v.len(),
            Self::Refs(v) => v.len(),
            Self::Values(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Byte cost of the payload, for heap accounting.
    pub fn byte_size(&self) -> usize {
        match self {
            Self::Bool(v) => v.len(),
            Self::I8(v) => v.len(),
            Self::U8(v) => v.len(),
            Self::I16(v) => v.len() * 2,
            Self::U16(v) | Self::Char(v) => v.len() * 2,
            Self::I32(v) => v.len() * 4,
            Self::U32(v) => v.len() * 4,
            Self::I64(v) => v.len() * 8,
            Self::U64(v) => v.len() * 8,
            Self::F32(v) => v.len() * 4,
            Self::F64(v) => v.len() * 8,
            Self::Refs(v) => v.len() * core::mem::size_of::<Handle>(),
            Self::Values(v) => v.len() * core::mem::size_of::<Value>(),
        }
    }

    /// The backing vector, when this is untyped `Values` storage.
    ///
    /// The generic collections resize their storage in place rather than
    /// reallocating an array per growth, which needs the vector itself. Every
    /// other storage kind has a fixed element width and is not resizable this
    /// way, so they answer `None`.
    pub fn values_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Self::Values(v) => Some(v),
            _ => None,
        }
    }

    /// Reads element `i` as an evaluation-stack value.
    pub fn get(&self, i: usize) -> Option<Value> {
        Some(match self {
            Self::Bool(v) => Value::I32(*v.get(i)? as i32),
            Self::I8(v) => Value::I32(*v.get(i)? as i32),
            Self::U8(v) => Value::I32(*v.get(i)? as i32),
            Self::I16(v) => Value::I32(*v.get(i)? as i32),
            Self::U16(v) | Self::Char(v) => Value::I32(*v.get(i)? as i32),
            Self::I32(v) => Value::I32(*v.get(i)?),
            Self::U32(v) => Value::I32(*v.get(i)? as i32),
            Self::I64(v) => Value::I64(*v.get(i)?),
            Self::U64(v) => Value::I64(*v.get(i)? as i64),
            Self::F32(v) => Value::F(*v.get(i)? as f64),
            Self::F64(v) => Value::F(*v.get(i)?),
            Self::Refs(v) => {
                let h = *v.get(i)?;
                if h.is_null() { Value::Null } else { Value::Obj(h) }
            }
            Self::Values(v) => v.get(i)?.clone(),
        })
    }

    /// Writes element `i`, truncating to the storage width as `stelem` does.
    pub fn set(&mut self, i: usize, value: &Value) -> bool {
        macro_rules! put {
            ($vec:expr, $conv:expr) => {{
                match $vec.get_mut(i) {
                    Some(slot) => {
                        *slot = $conv;
                        true
                    }
                    None => false,
                }
            }};
        }
        match self {
            Self::Bool(v) => put!(v, value.as_i32().unwrap_or(0) != 0),
            Self::I8(v) => put!(v, value.as_i32().unwrap_or(0) as i8),
            Self::U8(v) => put!(v, value.as_i32().unwrap_or(0) as u8),
            Self::I16(v) => put!(v, value.as_i32().unwrap_or(0) as i16),
            Self::U16(v) | Self::Char(v) => put!(v, value.as_i32().unwrap_or(0) as u16),
            Self::I32(v) => put!(v, value.as_i32().unwrap_or(0)),
            Self::U32(v) => put!(v, value.as_i32().unwrap_or(0) as u32),
            Self::I64(v) => put!(v, value.as_i64().unwrap_or(0)),
            Self::U64(v) => put!(v, value.as_i64().unwrap_or(0) as u64),
            Self::F32(v) => put!(v, value.as_f64().unwrap_or(0.0) as f32),
            Self::F64(v) => put!(v, value.as_f64().unwrap_or(0.0)),
            Self::Refs(v) => put!(v, value.as_handle().unwrap_or(Handle::NULL)),
            Self::Values(v) => put!(v, value.clone()),
        }
    }
}

/// An array instance.
#[derive(Debug)]
pub struct ClrArray {
    pub array_type: TypeId,
    pub element_type: TypeId,
    pub storage: ArrayStorage,
    /// Lengths per dimension; a single entry for `T[]`.
    pub dimensions: Vec<u32>,
}

impl ClrArray {
    pub fn len(&self) -> usize {
        self.storage.len()
    }
    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }
}

impl GcObject for ClrArray {
    fn trace(&self, tracer: &mut Tracer) {
        match &self.storage {
            ArrayStorage::Refs(handles) => tracer.edges(handles.iter().copied()),
            ArrayStorage::Values(values) => {
                let mut handles = Vec::new();
                for v in values {
                    v.trace_handles(&mut handles);
                }
                tracer.edges(handles);
            }
            _ => {} // primitive storage holds no references
        }
    }

    fn size_hint(&self) -> usize {
        40 + self.storage.byte_size()
    }

    fn type_name(&self) -> &str {
        "array"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A `System.String`.
///
/// .NET strings are sequences of UTF-16 code units and `Length`, indexing and
/// `Substring` are all defined in those units. Storing UTF-8 would make those
/// operations either wrong or O(n), so the payload is `Vec<u16>` and conversion
/// to Rust strings happens only at the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClrString {
    pub units: Vec<u16>,
}

impl ClrString {
    pub fn from_str(s: &str) -> Self {
        Self { units: s.encode_utf16().collect() }
    }

    pub fn empty() -> Self {
        Self { units: Vec::new() }
    }

    pub fn to_rust_string(&self) -> String {
        String::from_utf16_lossy(&self.units)
    }

    /// `String.Length`: the number of UTF-16 code units.
    pub fn len(&self) -> usize {
        self.units.len()
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    pub fn concat(&self, other: &ClrString) -> ClrString {
        let mut units = Vec::with_capacity(self.units.len() + other.units.len());
        units.extend_from_slice(&self.units);
        units.extend_from_slice(&other.units);
        ClrString { units }
    }

    pub fn char_at(&self, index: usize) -> Option<u16> {
        self.units.get(index).copied()
    }
}

impl GcObject for ClrString {
    fn trace(&self, _tracer: &mut Tracer) {}

    fn size_hint(&self) -> usize {
        24 + self.units.len() * 2
    }

    fn type_name(&self) -> &str {
        "string"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A boxed value type.
#[derive(Debug)]
pub struct ClrBox {
    pub type_id: TypeId,
    pub value: Value,
}

impl GcObject for ClrBox {
    fn trace(&self, tracer: &mut Tracer) {
        let mut handles = Vec::new();
        self.value.trace_handles(&mut handles);
        tracer.edges(handles);
    }

    fn size_hint(&self) -> usize {
        24 + core::mem::size_of::<Value>()
    }

    fn type_name(&self) -> &str {
        "box"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A delegate instance, including its invocation list for multicast.
#[derive(Debug)]
pub struct ClrDelegate {
    pub type_id: TypeId,
    /// One entry per target. A unicast delegate has exactly one.
    pub targets: Vec<DelegateTarget>,
}

#[derive(Debug, Clone, Copy)]
pub struct DelegateTarget {
    /// `null` for a static method.
    pub receiver: Handle,
    pub method: MethodId,
}

impl GcObject for ClrDelegate {
    fn trace(&self, tracer: &mut Tracer) {
        tracer.edges(self.targets.iter().map(|t| t.receiver));
    }

    fn size_hint(&self) -> usize {
        32 + self.targets.len() * 16
    }

    fn type_name(&self) -> &str {
        "delegate"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A managed exception object.
#[derive(Debug)]
pub struct ClrException {
    pub type_id: TypeId,
    pub message: String,
    pub inner: Handle,
    /// Frames captured when the exception was thrown.
    pub stack_trace: Vec<String>,
}

impl GcObject for ClrException {
    fn trace(&self, tracer: &mut Tracer) {
        tracer.edge(self.inner);
    }

    fn size_hint(&self) -> usize {
        64 + self.message.len() + self.stack_trace.iter().map(|s| s.len() + 16).sum::<usize>()
    }

    fn type_name(&self) -> &str {
        "exception"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_arrays_use_compact_storage() {
        let bytes = ArrayStorage::zeroed(Some(Primitive::Byte), false, 1000);
        assert_eq!(bytes.byte_size(), 1000);
        assert!(matches!(bytes, ArrayStorage::U8(_)));
    }

    #[test]
    fn storing_into_a_byte_array_truncates_like_stelem_i1() {
        let mut a = ArrayStorage::zeroed(Some(Primitive::Byte), false, 4);
        assert!(a.set(0, &Value::I32(300)));
        assert_eq!(a.get(0), Some(Value::I32(44)));
    }

    #[test]
    fn out_of_range_access_reports_failure_instead_of_panicking() {
        let mut a = ArrayStorage::zeroed(Some(Primitive::Int32), false, 2);
        assert_eq!(a.get(5), None);
        assert!(!a.set(5, &Value::I32(1)));
    }

    #[test]
    fn string_length_counts_utf16_code_units() {
        let s = ClrString::from_str("héllo");
        assert_eq!(s.len(), 5);
        assert_eq!(s.to_rust_string(), "héllo");

        // An astral-plane character is two UTF-16 units, as in .NET.
        let emoji = ClrString::from_str("\u{1F600}");
        assert_eq!(emoji.len(), 2);
    }

    #[test]
    fn reference_arrays_report_their_elements_to_the_collector() {
        let array = ClrArray {
            array_type: TypeId(1),
            element_type: TypeId(2),
            storage: ArrayStorage::Refs(vec![
                Handle::from_bits(0x1_0000_0001),
                Handle::NULL,
                Handle::from_bits(0x2_0000_0001),
            ]),
            dimensions: vec![3],
        };
        let mut tracer = Tracer::default();
        array.trace(&mut tracer);
        // Nulls are dropped by the tracer.
        assert_eq!(tracer.len(), 2);
    }
}
