//! Swappable collection policies.
//!
//! The requirement that the GC be replaceable is met by making collection a
//! trait: [`Collector`]. The runtime holds a `Box<dyn Collector>` and never
//! depends on which one is installed.

use crate::handle::Handle;
use crate::object::Tracer;
use crate::space::ObjectSpace;
use alloc::vec::Vec;

/// Supplies the roots of the object graph: interpreter stacks, static fields,
/// interop handles, and anything else that keeps objects alive.
pub trait RootSet {
    /// Appends every root handle to `out`.
    fn collect_roots(&self, out: &mut Vec<Handle>);
}

/// A root set backed by a plain slice, useful for tests and simple embedders.
impl RootSet for [Handle] {
    fn collect_roots(&self, out: &mut Vec<Handle>) {
        out.extend_from_slice(self);
    }
}

impl RootSet for Vec<Handle> {
    fn collect_roots(&self, out: &mut Vec<Handle>) {
        out.extend_from_slice(self);
    }
}

/// What one collection accomplished.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollectionReport {
    pub objects_freed: usize,
    pub bytes_freed: usize,
    pub objects_surviving: usize,
    pub bytes_surviving: usize,
    pub roots_scanned: usize,
}

/// Heap facts a collector consults to decide whether to run.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeapPressure {
    pub live_bytes: usize,
    pub live_count: usize,
    pub bytes_since_last_collection: usize,
}

/// A collection policy.
pub trait Collector: Send + Sync {
    fn name(&self) -> &'static str;

    /// Whether an allocation should trigger a collection now.
    fn should_collect(&self, pressure: &HeapPressure) -> bool;

    /// Reclaims unreachable objects.
    fn collect(&mut self, space: &mut ObjectSpace, roots: &dyn RootSet) -> CollectionReport;
}

/// Classic tracing mark-and-sweep.
///
/// Marking is iterative rather than recursive: a deep object graph must not
/// blow the native stack, which is a real failure mode on the microcontroller
/// targets this runtime supports.
#[derive(Debug)]
pub struct MarkSweep {
    /// Collect once this many bytes have been allocated since the last run.
    pub allocation_trigger: usize,
    /// Never collect below this live size; avoids thrashing on tiny heaps.
    pub minimum_heap: usize,
    /// Growth factor applied to the trigger after each collection.
    pub growth_factor: f32,
}

impl Default for MarkSweep {
    fn default() -> Self {
        Self {
            allocation_trigger: 4 * 1024 * 1024,
            minimum_heap: 256 * 1024,
            growth_factor: 1.5,
        }
    }
}

impl MarkSweep {
    /// A configuration tuned for memory-constrained targets.
    pub fn embedded() -> Self {
        Self {
            allocation_trigger: 32 * 1024,
            minimum_heap: 8 * 1024,
            growth_factor: 1.2,
        }
    }
}

impl Collector for MarkSweep {
    fn name(&self) -> &'static str {
        "mark-sweep"
    }

    fn should_collect(&self, pressure: &HeapPressure) -> bool {
        pressure.live_bytes > self.minimum_heap
            && pressure.bytes_since_last_collection >= self.allocation_trigger
    }

    fn collect(&mut self, space: &mut ObjectSpace, roots: &dyn RootSet) -> CollectionReport {
        space.clear_marks();

        // --- mark ------------------------------------------------------------
        let mut worklist = Vec::new();
        roots.collect_roots(&mut worklist);
        let roots_scanned = worklist.len();
        // Pinned objects are roots too: native code may hold them.
        worklist.extend(space.pinned_handles());

        let mut tracer = Tracer::default();
        while let Some(handle) = worklist.pop() {
            if !space.mark(handle) {
                continue; // null, stale, or already marked
            }
            space.trace_into(handle, &mut tracer);
            worklist.append(&mut tracer.take());
        }

        // --- sweep -----------------------------------------------------------
        let (objects_freed, bytes_freed) = space.sweep();

        // Widen the trigger so a program with a large live set does not spend
        // all its time collecting.
        let next = (self.allocation_trigger as f32 * self.growth_factor) as usize;
        self.allocation_trigger = next.max(space.live_bytes()).min(usize::MAX / 2);

        CollectionReport {
            objects_freed,
            bytes_freed,
            objects_surviving: space.live_count(),
            bytes_surviving: space.live_bytes(),
            roots_scanned,
        }
    }
}

/// A collector that never reclaims anything.
///
/// Appropriate for short-lived programs and for deterministic embedded
/// workloads where a collection pause is unacceptable and the heap is sized to
/// the whole run.
#[derive(Debug, Default)]
pub struct NeverCollect;

impl Collector for NeverCollect {
    fn name(&self) -> &'static str {
        "never-collect"
    }

    fn should_collect(&self, _pressure: &HeapPressure) -> bool {
        false
    }

    fn collect(&mut self, space: &mut ObjectSpace, _roots: &dyn RootSet) -> CollectionReport {
        CollectionReport {
            objects_surviving: space.live_count(),
            bytes_surviving: space.live_bytes(),
            ..Default::default()
        }
    }
}
