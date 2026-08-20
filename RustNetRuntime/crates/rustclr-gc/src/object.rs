//! The contract a heap object must satisfy.

use crate::handle::Handle;
use alloc::vec::Vec;
use core::any::Any;

/// Collects the outgoing references of an object during the mark phase.
#[derive(Debug, Default)]
pub struct Tracer {
    pub(crate) pending: Vec<Handle>,
}

impl Tracer {
    /// Reports one outgoing reference. Null handles are ignored.
    #[inline]
    pub fn edge(&mut self, handle: Handle) {
        if !handle.is_null() {
            self.pending.push(handle);
        }
    }

    /// Reports many outgoing references.
    #[inline]
    pub fn edges(&mut self, handles: impl IntoIterator<Item = Handle>) {
        for h in handles {
            self.edge(h);
        }
    }

    /// Number of edges reported so far.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// The edges reported so far, for tests and heap-graph tooling.
    pub fn edges_reported(&self) -> &[Handle] {
        &self.pending
    }

    pub(crate) fn take(&mut self) -> Vec<Handle> {
        core::mem::take(&mut self.pending)
    }
}

/// Anything that can live on the managed heap.
///
/// Implementors must report *every* handle they hold from [`GcObject::trace`].
/// Missing an edge causes a live object to be collected; reporting a stale
/// handle is harmless because the collector validates before following it.
pub trait GcObject: Any + Send + Sync {
    /// Reports outgoing references to the collector.
    fn trace(&self, tracer: &mut Tracer);

    /// Approximate retained size in bytes, used for heap accounting and to
    /// decide when a collection is due.
    ///
    /// The default is a nominal per-object cost. Types with variable-sized
    /// payloads (arrays, strings) should override it, or the collector will
    /// under-estimate pressure and run too rarely.
    fn size_hint(&self) -> usize {
        32
    }

    /// Human-readable type name, for diagnostics and heap dumps.
    fn type_name(&self) -> &str {
        "object"
    }

    /// Upcast helper so the heap can hand out concrete types.
    fn as_any(&self) -> &dyn Any;

    /// Mutable upcast helper.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Implements the two `Any` upcasts, which are pure boilerplate.
#[macro_export]
macro_rules! impl_gc_upcasts {
    () => {
        fn as_any(&self) -> &dyn ::core::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn ::core::any::Any {
            self
        }
    };
}

/// A leaf object holding no references, e.g. a byte buffer or a boxed integer.
impl GcObject for Vec<u8> {
    fn trace(&self, _tracer: &mut Tracer) {}

    fn size_hint(&self) -> usize {
        self.len()
    }

    fn type_name(&self) -> &str {
        "byte[]"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
