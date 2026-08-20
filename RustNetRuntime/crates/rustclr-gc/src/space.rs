//! The object space: slot storage behind the handle table.

use crate::handle::Handle;
use crate::object::{GcObject, Tracer};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::num::NonZeroU32;

pub(crate) struct Slot {
    /// Bumped on every reuse so stale handles fail validation.
    generation: NonZeroU32,
    object: Option<Box<dyn GcObject>>,
    pub(crate) marked: bool,
    /// Non-zero while native code holds the object; suppresses collection.
    pub(crate) pins: u32,
    bytes: usize,
}

/// Storage for every live object, addressed by [`Handle`].
pub struct ObjectSpace {
    slots: Vec<Slot>,
    /// Indices of slots whose object has been freed.
    free_list: Vec<u32>,
    live_bytes: usize,
    live_count: usize,
    /// Hard ceiling on slots, for a fixed heap. `None` means grow on demand.
    limit: Option<usize>,
}

impl Default for ObjectSpace {
    fn default() -> Self {
        Self::new()
    }
}

impl ObjectSpace {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_list: Vec::new(),
            live_bytes: 0,
            live_count: 0,
            limit: None,
        }
    }

    /// Pre-reserves capacity, useful on embedded targets where the heap size is
    /// fixed up front.
    pub fn with_capacity(slots: usize) -> Self {
        Self {
            slots: Vec::with_capacity(slots),
            free_list: Vec::with_capacity(slots / 4),
            live_bytes: 0,
            live_count: 0,
            limit: None,
        }
    }

    /// Pre-reserves capacity *and* refuses to exceed it.
    ///
    /// Reserving alone is only a performance hint — the vector still grows on
    /// demand, which on a microcontroller means quietly running past the RAM
    /// that was budgeted. A fixed heap has to be able to say no.
    pub fn fixed(slots: usize) -> Self {
        let mut space = Self::with_capacity(slots);
        space.limit = Some(slots);
        space
    }

    /// The hard ceiling on slots, if there is one.
    #[inline]
    pub fn limit(&self) -> Option<usize> {
        self.limit
    }

    /// Whether another object would fit without growing past the limit.
    #[inline]
    pub fn has_room(&self) -> bool {
        match self.limit {
            Some(limit) => !self.free_list.is_empty() || self.slots.len() < limit,
            None => true,
        }
    }

    #[inline]
    pub fn live_bytes(&self) -> usize {
        self.live_bytes
    }

    #[inline]
    pub fn live_count(&self) -> usize {
        self.live_count
    }

    #[inline]
    pub fn slot_capacity(&self) -> usize {
        self.slots.len()
    }

    /// Allocates an object and returns its handle.
    pub fn alloc(&mut self, object: Box<dyn GcObject>) -> Handle {
        self.try_alloc(object).expect("heap exhausted; call try_alloc on a fixed heap")
    }

    /// Allocates, or reports that a fixed heap is full.
    ///
    /// Only a heap built with [`Self::fixed`] can answer `None`; an unbounded
    /// one always succeeds or aborts on the host allocator.
    pub fn try_alloc(&mut self, object: Box<dyn GcObject>) -> Option<Handle> {
        if !self.has_room() {
            return None;
        }
        Some(self.alloc_unchecked(object))
    }

    fn alloc_unchecked(&mut self, object: Box<dyn GcObject>) -> Handle {
        let bytes = object.size_hint();
        self.live_bytes += bytes;
        self.live_count += 1;

        if let Some(index) = self.free_list.pop() {
            let slot = &mut self.slots[index as usize];
            // Reusing a slot invalidates every handle that pointed at it.
            slot.generation = slot.generation.checked_add(1).unwrap_or(NonZeroU32::MIN);
            slot.object = Some(object);
            slot.marked = false;
            slot.pins = 0;
            slot.bytes = bytes;
            return Handle::new(index, slot.generation);
        }

        let index = self.slots.len() as u32;
        let generation = NonZeroU32::MIN;
        self.slots.push(Slot {
            generation,
            object: Some(object),
            marked: false,
            pins: 0,
            bytes,
        });
        Handle::new(index, generation)
    }

    #[inline]
    fn slot(&self, handle: Handle) -> Option<&Slot> {
        if handle.is_null() {
            return None;
        }
        let slot = self.slots.get(handle.index())?;
        (slot.generation.get() == handle.generation() && slot.object.is_some()).then_some(slot)
    }

    #[inline]
    fn slot_mut(&mut self, handle: Handle) -> Option<&mut Slot> {
        if handle.is_null() {
            return None;
        }
        let want_generation = handle.generation();
        let slot = self.slots.get_mut(handle.index())?;
        (slot.generation.get() == want_generation && slot.object.is_some()).then_some(slot)
    }

    /// True when the handle still refers to a live object.
    #[inline]
    pub fn is_valid(&self, handle: Handle) -> bool {
        self.slot(handle).is_some()
    }

    /// Borrows an object.
    pub fn get(&self, handle: Handle) -> Option<&dyn GcObject> {
        self.slot(handle)?.object.as_deref()
    }

    /// Mutably borrows an object.
    pub fn get_mut(&mut self, handle: Handle) -> Option<&mut (dyn GcObject + 'static)> {
        self.slot_mut(handle)?.object.as_deref_mut()
    }

    /// Borrows an object as a concrete type.
    pub fn get_as<T: GcObject>(&self, handle: Handle) -> Option<&T> {
        self.get(handle)?.as_any().downcast_ref::<T>()
    }

    /// Mutably borrows an object as a concrete type.
    pub fn get_as_mut<T: GcObject>(&mut self, handle: Handle) -> Option<&mut T> {
        self.get_mut(handle)?.as_any_mut().downcast_mut::<T>()
    }

    /// Pins an object so the collector will not reclaim it. Used while native
    /// code holds a reference across a P/Invoke boundary.
    pub fn pin(&mut self, handle: Handle) -> bool {
        match self.slot_mut(handle) {
            Some(s) => {
                s.pins = s.pins.saturating_add(1);
                true
            }
            None => false,
        }
    }

    /// Releases one pin.
    pub fn unpin(&mut self, handle: Handle) -> bool {
        match self.slot_mut(handle) {
            Some(s) => {
                s.pins = s.pins.saturating_sub(1);
                true
            }
            None => false,
        }
    }

    pub fn is_pinned(&self, handle: Handle) -> bool {
        self.slot(handle).is_some_and(|s| s.pins > 0)
    }

    // -- collector-facing operations ----------------------------------------

    pub(crate) fn clear_marks(&mut self) {
        for slot in &mut self.slots {
            slot.marked = false;
        }
    }

    /// Marks a handle, returning true if this was the first time (so its
    /// children still need visiting).
    pub(crate) fn mark(&mut self, handle: Handle) -> bool {
        match self.slot_mut(handle) {
            Some(s) if !s.marked => {
                s.marked = true;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn trace_into(&self, handle: Handle, tracer: &mut Tracer) {
        if let Some(obj) = self.get(handle) {
            obj.trace(tracer);
        }
    }

    /// Handles of every pinned object, which are implicit roots.
    pub(crate) fn pinned_handles(&self) -> Vec<Handle> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.pins > 0 && s.object.is_some())
            .map(|(i, s)| Handle::new(i as u32, s.generation))
            .collect()
    }

    /// Frees every unmarked slot. Returns (objects freed, bytes freed).
    pub(crate) fn sweep(&mut self) -> (usize, usize) {
        let mut freed = 0usize;
        let mut bytes = 0usize;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.object.is_some() && !slot.marked {
                slot.object = None;
                bytes += slot.bytes;
                slot.bytes = 0;
                freed += 1;
                self.free_list.push(index as u32);
            }
        }
        self.live_bytes = self.live_bytes.saturating_sub(bytes);
        self.live_count = self.live_count.saturating_sub(freed);
        (freed, bytes)
    }

    /// Iterates live handles. Order is unspecified.
    pub fn iter_handles(&self) -> impl Iterator<Item = Handle> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.object.is_some())
            .map(|(i, s)| Handle::new(i as u32, s.generation))
    }
}

impl core::fmt::Debug for ObjectSpace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ObjectSpace")
            .field("live_count", &self.live_count)
            .field("live_bytes", &self.live_bytes)
            .field("slots", &self.slots.len())
            .field("free", &self.free_list.len())
            .finish()
    }
}

#[cfg(test)]
mod fixed_tests {
    use super::*;

    struct Blob;
    impl GcObject for Blob {
        fn trace(&self, _t: &mut Tracer) {}
        fn size_hint(&self) -> usize {
            8
        }
        fn type_name(&self) -> &str {
            "blob"
        }
        fn as_any(&self) -> &dyn core::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
    }

    #[test]
    fn a_fixed_space_refuses_to_grow_past_its_ceiling() {
        let mut space = ObjectSpace::fixed(3);
        assert_eq!(space.limit(), Some(3));
        for i in 0..3 {
            assert!(space.try_alloc(Box::new(Blob)).is_some(), "slot {i} should fit");
        }
        assert!(!space.has_room());
        assert!(
            space.try_alloc(Box::new(Blob)).is_none(),
            "a fixed heap must say no rather than growing past its budget"
        );
    }

    #[test]
    fn an_unbounded_space_never_refuses() {
        let mut space = ObjectSpace::new();
        assert_eq!(space.limit(), None);
        for _ in 0..64 {
            assert!(space.try_alloc(Box::new(Blob)).is_some());
        }
        assert!(space.has_room());
    }
}
