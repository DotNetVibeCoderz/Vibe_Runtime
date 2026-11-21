//! Swappable collection policies.
//!
//! The requirement that the GC be replaceable is met by making collection a
//! trait: [`Collector`]. The runtime holds a `Box<dyn Collector>` and never
//! depends on which one is installed.

use crate::handle::Handle;
use crate::object::Tracer;
use crate::space::{route, ObjectSpace};
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
    /// Slots the heap may ever hold, on a heap that is bounded by slots.
    ///
    /// A bounded heap can run out two ways, and for a long time a policy could
    /// only see one of them. Bytes are what a desktop runs out of; a board
    /// hits the **slot ceiling** first, because the ceiling is paid for up
    /// front and sized to fit the RAM rather than to fit the program. A loop
    /// allocating short-lived 12-byte objects fills 512 slots while live bytes
    /// stay under any sane byte threshold, so a byte-only policy is never
    /// asked, the ceiling is reached, and the allocation fails on a heap that
    /// was almost entirely garbage.
    ///
    /// That is not hypothetical: it killed `Showcase` on a Maix Go after two
    /// passes through its scene loop.
    pub slot_limit: Option<usize>,
}

impl HeapPressure {
    /// How full the slot table is, from 0.0 to 1.0, on a bounded heap.
    ///
    /// `None` on an unbounded one — there is no ceiling to be near.
    pub fn slot_occupancy(&self) -> Option<f32> {
        let limit = self.slot_limit?;
        if limit == 0 {
            return Some(1.0);
        }
        Some(self.live_count as f32 / limit as f32)
    }
}

/// A collection policy.
pub trait Collector: Send + Sync {
    fn name(&self) -> &'static str;

    /// Whether an allocation should trigger a collection now.
    fn should_collect(&self, pressure: &HeapPressure) -> bool;

    /// Reclaims unreachable objects.
    ///
    /// `spaces` is the whole heap: one space, or every shard of a sharded one
    /// in shard order. Objects in one shard point at objects in another, so a
    /// collector has to see them all at once.
    fn collect(&mut self, spaces: &mut [&mut ObjectSpace], roots: &dyn RootSet) -> CollectionReport;
}

/// Classic tracing mark-and-sweep.
///
/// Marking is iterative rather than recursive: a deep object graph must not
/// blow the native stack, which is a real failure mode on the microcontroller
/// targets this runtime supports.
#[derive(Debug)]
pub struct MarkSweep {
    /// Collect once this many bytes have been allocated since the last run.
    /// Grows by `growth_factor` after each collection, within the bounds
    /// [`Self::next_trigger`] sets.
    pub allocation_trigger: usize,
    /// Where the trigger starts, and the lowest it returns to.
    pub base_trigger: usize,
    /// Never collect below this live size; avoids thrashing on tiny heaps.
    pub minimum_heap: usize,
    /// How much the trigger grows after each collection.
    pub growth_factor: f32,
}

impl Default for MarkSweep {
    fn default() -> Self {
        Self {
            allocation_trigger: 4 * 1024 * 1024,
            base_trigger: 4 * 1024 * 1024,
            minimum_heap: 256 * 1024,
            growth_factor: 1.5,
        }
    }
}

impl MarkSweep {
    /// How far past the live set the trigger may grow.
    ///
    /// The trigger grows after every collection, which is what keeps a
    /// program that allocates heavily from collecting constantly — `fanout`
    /// ran 40% slower when the growth was taken away. But it used to grow
    /// *whatever survived*: a channel stream whose live set never passed
    /// 3.5 KB had a 31 MB trigger after six collections, and forty
    /// collections in, any program's trigger would be terabytes — a server
    /// that runs for a day stops collecting at all. Bounded by a multiple of
    /// what survived, it still grows for a program that keeps a lot alive and
    /// comes back down for one that does not.
    pub const MAX_TRIGGER_PER_LIVE_BYTE: usize = 64;

    /// The trigger after a collection that left `live_bytes` alive: grown by
    /// `growth_factor`, at most [`Self::MAX_TRIGGER_PER_LIVE_BYTE`] times the
    /// live set, and never below `base_trigger`.
    pub fn next_trigger(&self, live_bytes: usize) -> usize {
        let grown = (self.allocation_trigger as f32 * self.growth_factor) as usize;
        let ceiling = live_bytes.saturating_mul(Self::MAX_TRIGGER_PER_LIVE_BYTE);
        grown.min(ceiling).max(self.base_trigger).min(usize::MAX / 2)
    }

    /// How full a bounded slot table gets before a collection is asked for.
    ///
    /// Three quarters, not the last slot. Collecting at the ceiling means the
    /// allocation that triggered it has nowhere to go if the collection frees
    /// nothing, and leaves no headroom for the collector's own bookkeeping.
    /// Three quarters of 512 slots is 128 slots of room, which is more than a
    /// mark phase needs and enough that a program whose live set genuinely is
    /// that large still gets a straight answer instead of thrashing.
    pub const SLOT_TRIGGER: f32 = 0.75;

    /// A configuration tuned for memory-constrained targets.
    pub fn embedded() -> Self {
        Self {
            allocation_trigger: 32 * 1024,
            base_trigger: 32 * 1024,
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
        // Running low on slots is pressure on its own, and it deliberately
        // ignores `minimum_heap`: that threshold exists to stop a tiny heap
        // thrashing, and a heap this close to its ceiling is not a heap with
        // room to spare. See [`HeapPressure::slot_limit`] for what goes wrong
        // without this.
        if pressure.slot_occupancy().is_some_and(|used| used >= Self::SLOT_TRIGGER) {
            return true;
        }
        pressure.live_bytes > self.minimum_heap
            && pressure.bytes_since_last_collection >= self.allocation_trigger
    }

    fn collect(&mut self, spaces: &mut [&mut ObjectSpace], roots: &dyn RootSet) -> CollectionReport {
        for space in spaces.iter_mut() {
            space.clear_marks();
        }

        // --- mark ------------------------------------------------------------
        let mut worklist = Vec::new();
        roots.collect_roots(&mut worklist);
        let roots_scanned = worklist.len();
        // Pinned objects are roots too: native code may hold them.
        for space in spaces.iter() {
            worklist.extend(space.pinned_handles());
        }

        let mut tracer = Tracer::default();
        while let Some(handle) = worklist.pop() {
            let Some(space) = route(spaces, handle) else { continue };
            if !space.mark(handle) {
                continue; // null, stale, or already marked
            }
            space.trace_into(handle, &mut tracer);
            worklist.append(&mut tracer.take());
        }

        // --- sweep -----------------------------------------------------------
        let (mut objects_freed, mut bytes_freed) = (0, 0);
        for space in spaces.iter_mut() {
            let (objects, bytes) = space.sweep();
            objects_freed += objects;
            bytes_freed += bytes;
        }
        let (live_count, live_bytes) = totals(spaces);

        // Widen the trigger so a program that allocates heavily does not
        // spend all its time collecting — within a bound set by what is live.
        self.allocation_trigger = self.next_trigger(live_bytes);

        CollectionReport {
            objects_freed,
            bytes_freed,
            objects_surviving: live_count,
            bytes_surviving: live_bytes,
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

    fn collect(&mut self, spaces: &mut [&mut ObjectSpace], _roots: &dyn RootSet) -> CollectionReport {
        let (live_count, live_bytes) = totals(spaces);
        CollectionReport {
            objects_surviving: live_count,
            bytes_surviving: live_bytes,
            ..Default::default()
        }
    }
}

/// Live objects and bytes, across every space.
fn totals(spaces: &[&mut ObjectSpace]) -> (usize, usize) {
    spaces.iter().fold((0, 0), |(count, bytes), s| (count + s.live_count(), bytes + s.live_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program that keeps almost nothing alive must not see its trigger
    /// compound. Before the bound, six collections of a 3.5 KB live set took
    /// the trigger from 4 MB to 31 MB, and it would have kept going.
    #[test]
    fn a_small_live_set_keeps_the_trigger_at_its_base() {
        let mut policy = MarkSweep::default();
        for _ in 0..40 {
            policy.allocation_trigger = policy.next_trigger(3_500);
        }
        assert_eq!(policy.allocation_trigger, policy.base_trigger);
    }

    /// A program that keeps a lot alive still gets a growing trigger, which
    /// is what keeps it from collecting after every few megabytes.
    #[test]
    fn a_large_live_set_lets_the_trigger_grow() {
        let mut policy = MarkSweep::default();
        let live = 20 * 1024 * 1024;
        let mut previous = policy.allocation_trigger;
        for _ in 0..5 {
            policy.allocation_trigger = policy.next_trigger(live);
            assert!(policy.allocation_trigger > previous, "the trigger grows with a large live set");
            previous = policy.allocation_trigger;
        }
        // ...but never past the bound.
        for _ in 0..40 {
            policy.allocation_trigger = policy.next_trigger(live);
        }
        assert_eq!(policy.allocation_trigger, live * MarkSweep::MAX_TRIGGER_PER_LIVE_BYTE);
    }

    /// After a spike the trigger comes back down once the live set does.
    #[test]
    fn the_trigger_shrinks_when_the_live_set_does() {
        let mut policy = MarkSweep::default();
        for _ in 0..20 {
            policy.allocation_trigger = policy.next_trigger(50 * 1024 * 1024);
        }
        let spiked = policy.allocation_trigger;
        policy.allocation_trigger = policy.next_trigger(10 * 1024);
        assert!(policy.allocation_trigger < spiked);
        assert_eq!(policy.allocation_trigger, policy.base_trigger);
    }
}
