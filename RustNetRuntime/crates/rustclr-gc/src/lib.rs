//! # rustclr-gc
//!
//! The managed heap for [RustCLR], with a swappable collection policy.
//!
//! Two decisions shape this crate:
//!
//! 1. **Handles, not pointers.** Objects are reached through a generation-
//!    tagged handle table ([`Handle`]). A stale reference is reported as
//!    invalid instead of dereferencing freed memory, and the whole object graph
//!    is expressible without `unsafe`.
//! 2. **Collection is a trait.** [`Collector`] is the seam that makes the GC
//!    replaceable, as the runtime requirements demand. [`MarkSweep`] is the
//!    default; [`NeverCollect`] suits short-lived or hard-real-time programs.
//!
//! ```
//! use rustclr_gc::{Heap, GcObject, Tracer, Handle};
//! use std::any::Any;
//!
//! struct Node { next: Handle }
//! impl GcObject for Node {
//!     fn trace(&self, t: &mut Tracer) { t.edge(self.next); }
//!     fn as_any(&self) -> &dyn Any { self }
//!     fn as_any_mut(&mut self) -> &mut dyn Any { self }
//! }
//!
//! let mut heap = Heap::new();
//! let tail = heap.alloc(Node { next: Handle::NULL });
//! let head = heap.alloc(Node { next: tail });
//!
//! // Only `head` is a root, but `tail` is reachable from it.
//! let report = heap.collect(&vec![head]);
//! assert_eq!(report.objects_freed, 0);
//!
//! // Drop the root and both die.
//! let report = heap.collect(&Vec::new());
//! assert_eq!(report.objects_freed, 2);
//! ```
//!
//! [RustCLR]: https://github.com/gravicode/RustNetRuntime

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod collector;
pub mod handle;
pub mod object;
pub mod safepoint;
pub mod shared;
pub mod space;

pub use collector::{CollectionReport, Collector, HeapPressure, MarkSweep, NeverCollect, RootSet};
pub use shared::SharedHeap;
pub use handle::Handle;
pub use object::{GcObject, Tracer};
pub use space::ObjectSpace;

use alloc::boxed::Box;

/// Cumulative heap statistics, surfaced by the profiler and the CLI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcStats {
    pub total_allocations: u64,
    pub total_bytes_allocated: u64,
    pub collections: u64,
    pub total_objects_freed: u64,
    pub total_bytes_freed: u64,
    pub peak_live_bytes: usize,
    pub peak_live_count: usize,
}

/// The managed heap: object storage plus a collection policy.
pub struct Heap {
    space: ObjectSpace,
    collector: Box<dyn Collector>,
    bytes_since_collection: usize,
    stats: GcStats,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    /// A heap using the default [`MarkSweep`] policy.
    pub fn new() -> Self {
        Self::with_collector(Box::new(MarkSweep::default()))
    }

    /// A heap tuned for microcontroller targets.
    ///
    /// The capacity is a **hard ceiling**, not a hint: allocation past it
    /// fails rather than growing. That is the whole point on a device whose
    /// RAM was budgeted up front — a heap that quietly grows has not been
    /// bounded at all. Use [`Self::try_alloc`] there, and treat `None` as the
    /// out-of-memory condition it is.
    pub fn embedded(slot_capacity: usize) -> Self {
        Self {
            space: ObjectSpace::fixed(slot_capacity),
            collector: Box::new(MarkSweep::embedded()),
            bytes_since_collection: 0,
            stats: GcStats::default(),
        }
    }

    /// The slot ceiling, if this heap has one.
    pub fn slot_limit(&self) -> Option<usize> {
        self.space.limit()
    }

    /// A heap with a caller-supplied policy.
    pub fn with_collector(collector: Box<dyn Collector>) -> Self {
        Self {
            space: ObjectSpace::new(),
            collector,
            bytes_since_collection: 0,
            stats: GcStats::default(),
        }
    }

    /// Swaps the collection policy at runtime, returning the previous one.
    pub fn set_collector(&mut self, collector: Box<dyn Collector>) -> Box<dyn Collector> {
        core::mem::replace(&mut self.collector, collector)
    }

    pub fn collector_name(&self) -> &'static str {
        self.collector.name()
    }

    /// Allocates an object.
    ///
    /// This never collects on its own — collection needs the root set, which
    /// only the runtime can supply. Call [`Heap::should_collect`] after
    /// allocating and run [`Heap::collect`] at a safe point.
    /// Allocates, or reports that a fixed heap is full.
    ///
    /// Returns `None` only for a heap built by [`Self::embedded`]. A caller
    /// that gets `None` should collect and try again; if it still fails, the
    /// heap really is exhausted.
    pub fn try_alloc<T: GcObject>(&mut self, object: T) -> Option<Handle> {
        let size = object.size_hint();
        let handle = self.space.try_alloc(Box::new(object))?;
        self.bytes_since_collection += size;
        self.stats.total_allocations += 1;
        self.stats.total_bytes_allocated += size as u64;
        self.stats.peak_live_bytes = self.stats.peak_live_bytes.max(self.space.live_bytes());
        self.stats.peak_live_count = self.stats.peak_live_count.max(self.space.live_count());
        Some(handle)
    }

    pub fn alloc<T: GcObject>(&mut self, object: T) -> Handle {
        let size = object.size_hint();
        let handle = self.space.alloc(Box::new(object));
        self.bytes_since_collection += size;
        self.stats.total_allocations += 1;
        self.stats.total_bytes_allocated += size as u64;
        self.stats.peak_live_bytes = self.stats.peak_live_bytes.max(self.space.live_bytes());
        self.stats.peak_live_count = self.stats.peak_live_count.max(self.space.live_count());
        handle
    }

    /// Whether the policy wants a collection at the next safe point.
    pub fn should_collect(&self) -> bool {
        self.collector.should_collect(&HeapPressure {
            live_bytes: self.space.live_bytes(),
            live_count: self.space.live_count(),
            bytes_since_last_collection: self.bytes_since_collection,
        })
    }

    /// Runs a collection with the given roots.
    pub fn collect(&mut self, roots: &dyn RootSet) -> CollectionReport {
        let report = self.collector.collect(&mut self.space, roots);
        self.bytes_since_collection = 0;
        self.stats.collections += 1;
        self.stats.total_objects_freed += report.objects_freed as u64;
        self.stats.total_bytes_freed += report.bytes_freed as u64;
        report
    }

    pub fn get(&self, handle: Handle) -> Option<&dyn GcObject> {
        self.space.get(handle)
    }

    pub fn get_mut(&mut self, handle: Handle) -> Option<&mut (dyn GcObject + 'static)> {
        self.space.get_mut(handle)
    }

    pub fn get_as<T: GcObject>(&self, handle: Handle) -> Option<&T> {
        self.space.get_as::<T>(handle)
    }

    pub fn get_as_mut<T: GcObject>(&mut self, handle: Handle) -> Option<&mut T> {
        self.space.get_as_mut::<T>(handle)
    }

    /// Reads an object through a closure rather than by handing out a borrow.
    ///
    /// This is the shape an accessor has to have once the heap is shared
    /// between threads: a `&T` returned from behind a lock would outlive the
    /// guard that made it safe, so the borrow has to stay inside a scope the
    /// heap controls.
    ///
    /// Returns `None` — without calling `f` — when the handle is stale or names
    /// something that is not a `T`, which is the same answer [`Self::get_as`]
    /// gives.
    ///
    /// The closure must not reach back into the heap. Nothing in this runtime
    /// does: of 56 accessor sites, none holds a borrow across a call that would
    /// need one, which is what makes a lock viable here at all.
    pub fn with<T: GcObject, R>(&self, handle: Handle, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.space.get_as::<T>(handle).map(f)
    }

    /// [`Self::with`], for a mutation.
    pub fn with_mut<T: GcObject, R>(
        &mut self,
        handle: Handle,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        self.space.get_as_mut::<T>(handle).map(f)
    }

    pub fn is_valid(&self, handle: Handle) -> bool {
        self.space.is_valid(handle)
    }

    pub fn pin(&mut self, handle: Handle) -> bool {
        self.space.pin(handle)
    }

    pub fn unpin(&mut self, handle: Handle) -> bool {
        self.space.unpin(handle)
    }

    pub fn is_pinned(&self, handle: Handle) -> bool {
        self.space.is_pinned(handle)
    }

    pub fn live_bytes(&self) -> usize {
        self.space.live_bytes()
    }

    pub fn live_count(&self) -> usize {
        self.space.live_count()
    }

    pub fn stats(&self) -> GcStats {
        self.stats
    }

    pub fn space(&self) -> &ObjectSpace {
        &self.space
    }
}

impl core::fmt::Debug for Heap {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Heap")
            .field("collector", &self.collector.name())
            .field("space", &self.space)
            .field("stats", &self.stats)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::any::Any;

    struct Cell {
        value: i32,
        next: Handle,
    }

    impl GcObject for Cell {
        fn trace(&self, t: &mut Tracer) {
            t.edge(self.next);
        }
        fn size_hint(&self) -> usize {
            16
        }
        fn type_name(&self) -> &str {
            "Cell"
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    #[test]
    fn reachable_objects_survive_and_garbage_is_freed() {
        let mut heap = Heap::new();
        let a = heap.alloc(Cell { value: 1, next: Handle::NULL });
        let b = heap.alloc(Cell { value: 2, next: a });
        let _orphan = heap.alloc(Cell { value: 3, next: Handle::NULL });

        let report = heap.collect(&vec![b]);
        assert_eq!(report.objects_freed, 1);
        assert!(heap.is_valid(a), "a is reachable through b");
        assert!(heap.is_valid(b));
        assert_eq!(heap.get_as::<Cell>(a).unwrap().value, 1);
    }

    #[test]
    fn a_cycle_is_collected_when_it_becomes_unreachable() {
        let mut heap = Heap::new();
        let a = heap.alloc(Cell { value: 1, next: Handle::NULL });
        let b = heap.alloc(Cell { value: 2, next: a });
        heap.get_as_mut::<Cell>(a).unwrap().next = b; // close the cycle

        assert_eq!(heap.collect(&vec![a]).objects_freed, 0);
        assert_eq!(heap.collect(&Vec::new()).objects_freed, 2);
        assert!(!heap.is_valid(a));
        assert!(!heap.is_valid(b));
    }

    #[test]
    fn a_stale_handle_is_rejected_rather_than_reused() {
        let mut heap = Heap::new();
        let doomed = heap.alloc(Cell { value: 9, next: Handle::NULL });
        heap.collect(&Vec::new());
        assert!(!heap.is_valid(doomed));

        // The slot is recycled, but the old handle must not resolve to the new
        // occupant.
        let fresh = heap.alloc(Cell { value: 10, next: Handle::NULL });
        assert!(heap.is_valid(fresh));
        assert!(!heap.is_valid(doomed), "generation counter must invalidate the old handle");
        assert_ne!(fresh, doomed);
    }

    #[test]
    fn pinned_objects_are_implicit_roots() {
        let mut heap = Heap::new();
        let pinned = heap.alloc(Cell { value: 5, next: Handle::NULL });
        assert!(heap.pin(pinned));

        assert_eq!(heap.collect(&Vec::new()).objects_freed, 0);
        assert!(heap.is_valid(pinned));

        heap.unpin(pinned);
        assert_eq!(heap.collect(&Vec::new()).objects_freed, 1);
    }

    #[test]
    fn marking_a_deep_chain_does_not_overflow_the_stack() {
        let mut heap = Heap::new();
        let mut head = Handle::NULL;
        for i in 0..200_000 {
            head = heap.alloc(Cell { value: i, next: head });
        }
        let report = heap.collect(&vec![head]);
        assert_eq!(report.objects_freed, 0);
        assert_eq!(report.objects_surviving, 200_000);
    }

    #[test]
    fn the_collector_can_be_swapped_at_runtime() {
        let mut heap = Heap::with_collector(Box::new(NeverCollect));
        let h = heap.alloc(Cell { value: 1, next: Handle::NULL });
        assert_eq!(heap.collector_name(), "never-collect");
        assert_eq!(heap.collect(&Vec::new()).objects_freed, 0);
        assert!(heap.is_valid(h));

        heap.set_collector(Box::new(MarkSweep::default()));
        assert_eq!(heap.collector_name(), "mark-sweep");
        assert_eq!(heap.collect(&Vec::new()).objects_freed, 1);
    }

    #[test]
    fn stats_track_allocation_and_reclamation() {
        let mut heap = Heap::new();
        for i in 0..10 {
            heap.alloc(Cell { value: i, next: Handle::NULL });
        }
        assert_eq!(heap.stats().total_allocations, 10);
        assert_eq!(heap.live_count(), 10);

        heap.collect(&Vec::new());
        let s = heap.stats();
        assert_eq!(s.collections, 1);
        assert_eq!(s.total_objects_freed, 10);
        assert_eq!(heap.live_count(), 0);
        assert_eq!(s.peak_live_count, 10);
    }
}
