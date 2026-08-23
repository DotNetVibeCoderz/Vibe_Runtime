//! Allocation from compiled code.
//!
//! The backend used to decline any method that allocates, and the reason was
//! not squeamishness: compiled code reaches no safe point, so a collection
//! running while it executes would have to walk a frame the collector cannot
//! read. It also holds raw pointers into array storage, which a collector that
//! moved objects would invalidate.
//!
//! # Why it is sound now
//!
//! Three facts, and the design needs all three:
//!
//! 1. **`Heap::alloc` does not collect.** It allocates and records pressure;
//!    the interpreter collects between instructions, and compiled code has no
//!    between-instructions. So allocating from compiled code cannot itself
//!    trigger a collection, and the existing argument for array pointers —
//!    that nothing collects during the call — still holds.
//!
//! 2. **A compiled method cannot return a reference.** Its return type must be
//!    an integer or a float, so every array it allocates is unreachable the
//!    moment the call ends. Nothing has to root them and nothing has to hand
//!    them back: the sink owns them for the call and drops them after.
//!
//! 3. **Allocation is bounded.** A `newarr` inside a loop is declined, so the
//!    number of allocations a compiled call can make is bounded by its
//!    instruction count. Without that, a loop allocating without collecting
//!    would exhaust the heap where the interpreter would not — a program that
//!    ran interpreted and failed compiled, which is the one difference this
//!    runtime will not accept.
//!
//! When the heap is genuinely full, [`alloc_array`] returns zero, compiled code
//! takes the trap path and the tier raises `OutOfMemoryException`.

use rustclr_core::gc::{Handle, SharedHeap};
use rustclr_core::{ArrayStorage, ClrArray, TypeId};

/// Element kinds compiled code can allocate, as passed in a register.
pub const ELEMENT_I4: i64 = 0;
pub const ELEMENT_R8: i64 = 1;
pub const ELEMENT_U1: i64 = 2;

/// The array types a compiled method may allocate, resolved once per call.
#[derive(Clone, Copy)]
pub struct ArrayTypes {
    pub int_array: TypeId,
    pub int_element: TypeId,
    pub double_array: TypeId,
    pub double_element: TypeId,
    pub bool_array: TypeId,
    pub bool_element: TypeId,
}

/// Arrays a compiled method allocated, alive for exactly one call.
///
/// Compiled code holds the *address* of a descriptor, so a descriptor must not
/// move once handed out — hence the boxes rather than a `Vec<[i64; 2]>` that
/// would reallocate as it grows.
pub struct AllocSink {
    heap: SharedHeap,
    types: ArrayTypes,
    /// Handles allocated during this call, dropped with the sink.
    allocated: Vec<Handle>,
    /// Descriptors, each at a stable address.
    descriptors: Vec<Box<[i64; 2]>>,
}

impl AllocSink {
    pub fn new(heap: SharedHeap, types: ArrayTypes) -> Self {
        Self { heap, types, allocated: Vec::new(), descriptors: Vec::new() }
    }

    /// How many arrays this call allocated, for tests and diagnostics.
    pub fn allocated(&self) -> usize {
        self.allocated.len()
    }

    fn make(&mut self, kind: i64, length: usize) -> Option<i64> {
        let (array_type, element_type, storage) = match kind {
            ELEMENT_I4 => (
                self.types.int_array,
                self.types.int_element,
                ArrayStorage::I32(vec![0; length]),
            ),
            ELEMENT_R8 => (
                self.types.double_array,
                self.types.double_element,
                ArrayStorage::F64(vec![0.0; length]),
            ),
            ELEMENT_U1 => (
                self.types.bool_array,
                self.types.bool_element,
                ArrayStorage::Bool(vec![false; length]),
            ),
            // The emitter only ever passes the kinds above; anything else
            // means the compiled code and this helper disagree, which is a bug
            // rather than a program error.
            _ => return None,
        };

        let handle = self.heap.try_alloc(ClrArray {
            array_type,
            element_type,
            storage,
            dimensions: vec![length as u32],
        })?;
        self.allocated.push(handle);

        // The descriptor compiled code reads: data pointer, then length.
        let descriptor = self
            .heap
            .with::<ClrArray, _>(handle, |array| match &array.storage {
                ArrayStorage::I32(v) => [v.as_ptr() as i64, v.len() as i64],
                ArrayStorage::F64(v) => [v.as_ptr() as i64, v.len() as i64],
                // `Vec<bool>` is one byte per element with the same layout a
                // byte buffer has, which is exactly what compiled code reads.
                ArrayStorage::Bool(v) => [v.as_ptr() as i64, v.len() as i64],
                _ => [0, 0],
            })?;
        if descriptor[0] == 0 {
            return None;
        }
        self.descriptors.push(Box::new(descriptor));
        Some(self.descriptors.last()?.as_ref().as_ptr() as i64)
    }
}

/// Allocates an array for compiled code.
///
/// Returns the address of a two-word descriptor, or **zero** when the heap
/// could not satisfy the request — compiled code branches on that and takes
/// the trap path, exactly as a failed bounds check does.
///
/// # Safety
///
/// `sink` must be the pointer the tier placed in the argument array for this
/// call, and the call must still be running. Compiled code reads it out of a
/// slot the tier wrote and never stores it anywhere, so it cannot outlive the
/// sink.
pub extern "C" fn alloc_array(sink: *mut AllocSink, kind: i64, length: i64) -> i64 {
    if sink.is_null() || length < 0 {
        return 0;
    }
    // SAFETY: the tier wrote this pointer into the argument slot compiled code
    // read it from, and owns the sink for the duration of the call.
    let sink = unsafe { &mut *sink };
    sink.make(kind, length as usize).unwrap_or(0)
}
