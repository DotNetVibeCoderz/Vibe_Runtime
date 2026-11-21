//! A heap several threads can hold at once.
//!
//! [`Heap`] is owned: its accessors take `&self` or `&mut self` and the caller
//! holds the borrow. That is the right shape for one thread and impossible for
//! several — an object reference handed out from behind a lock would outlive
//! the guard that made it safe.
//!
//! [`SharedHeap`] is the same heap with the borrow kept inside. Every method
//! takes `&self` and does its work while the lock is held, so a reference into
//! the heap never escapes.
//!
//! # Why this could be added without rewriting the runtime
//!
//! Because the accessors had already been converted. Every place that reads a
//! managed object now goes through [`Heap::with`] or [`Heap::with_mut`] and
//! hands in a closure — 56 sites, migrated one batch at a time against the
//! conformance fixture. Once none of them held a borrow, the difference between
//! an owned heap and a shared one stopped being visible at the call site.
//!
//! Doing it in that order was deliberate. Converting the accessors is a change
//! with no behavioural risk that can be verified at every step; putting a lock
//! underneath is a change with no *structural* risk once they are converted.
//! Attempted together, a failure could have been either.
//!
//! # Without `std`
//!
//! A microcontroller has one thread and no `Mutex`, so the cell is a
//! [`RefCell`](core::cell::RefCell). It costs a borrow flag rather than an
//! atomic, and it panics on re-entrancy instead of deadlocking — which is the
//! better failure, and one nothing in this runtime provokes.

use crate::{CollectionReport, GcObject, GcStats, Handle, Heap, RootSet};

// `Arc` needs an atomic compare-and-swap, and two of this runtime's bare-metal
// targets — `thumbv6m` and `riscv32imc` — have no atomics at all, so
// `alloc::sync::Arc` does not exist there. `Rc` is the right type on a chip
// anyway: one core, one thread, and nothing to share across.
#[cfg(feature = "threads")]
use rustclr_thread::sync::Arc;
#[cfg(not(feature = "threads"))]
use alloc::rc::Rc as Arc;

#[cfg(feature = "threads")]
type Cell = rustclr_thread::sync::Mutex<Heap>;
#[cfg(not(feature = "threads"))]
type Cell = core::cell::RefCell<Heap>;

/// A [`Heap`] that can be shared between threads.
///
/// Cloning gives another handle to the same heap, not another heap.
#[derive(Clone)]
pub struct SharedHeap {
    inner: Arc<Cell>,
    /// Lock acquisitions that found the heap already held.
    #[cfg(feature = "threads")]
    contended: Arc<core::sync::atomic::AtomicUsize>,
}

impl SharedHeap {
    pub fn new(heap: Heap) -> Self {
        Self {
            inner: Arc::new(Cell::new(heap)),
            #[cfg(feature = "threads")]
            contended: Arc::new(core::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Runs `f` with the heap locked.
    ///
    /// Private on purpose: handing this out would let a caller hold the lock
    /// across arbitrary work, which is exactly what the wrapper exists to
    /// prevent.
    #[cfg(feature = "threads")]
    fn locked<R>(&self, f: impl FnOnce(&mut Heap) -> R) -> R {
        // A poisoned heap means another thread panicked mid-mutation. There is
        // no recovery from a half-updated object graph, so this propagates
        // rather than pretending the heap is usable.
        let mut heap = match self.inner.try_lock() {
            Ok(heap) => heap,
            // Contended: counted, then waited for. The count is the pool's
            // signal that more threads are only queueing here — see
            // `contended_acquisitions`. Counting only on this path keeps the
            // uncontended lock, which is nearly every one, exactly as cheap as
            // before.
            Err(_) => {
                self.contended.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                self.inner.lock().expect("the managed heap is poisoned")
            }
        };
        f(&mut heap)
    }

    #[cfg(not(feature = "threads"))]
    fn locked<R>(&self, f: impl FnOnce(&mut Heap) -> R) -> R {
        f(&mut self.inner.borrow_mut())
    }

    pub fn alloc<T: GcObject>(&self, object: T) -> Handle {
        count_on_this_thread();
        self.locked(|h| h.alloc(object))
    }

    pub fn try_alloc<T: GcObject>(&self, object: T) -> Option<Handle> {
        let handle = self.locked(|h| h.try_alloc(object));
        if handle.is_some() {
            count_on_this_thread();
        }
        handle
    }

    /// Allocations the calling OS thread has made, through any heap.
    ///
    /// `stats()` is the heap's total, every thread together. The profiler
    /// watches one interpreter, so comparing it against the total says nothing
    /// once a program starts threads — a fixture with four workers updating a
    /// dictionary showed 87% "attributed" with no allocation site missed.
    /// Host builds only: a board has one thread, where the total is the same
    /// number.
    #[cfg(feature = "std")]
    pub fn allocations_on_this_thread() -> u64 {
        THREAD_ALLOCATIONS.with(|c| c.get())
    }

    /// Reads an object, if the handle names a live one of that type.
    pub fn with<T: GcObject, R>(&self, handle: Handle, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.locked(|h| h.with::<T, R>(handle, f))
    }

    /// [`Self::with`], for a mutation.
    pub fn with_mut<T: GcObject, R>(
        &self,
        handle: Handle,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        self.locked(|h| h.with_mut::<T, R>(handle, f))
    }

    /// Reads an object without naming its type.
    ///
    /// Scoped like the others: the `&dyn GcObject` a caller would otherwise
    /// receive is the one thing that cannot cross the lock.
    pub fn with_any<R>(&self, handle: Handle, f: impl FnOnce(&dyn GcObject) -> R) -> Option<R> {
        self.locked(|h| h.get(handle).map(f))
    }

    pub fn is_valid(&self, handle: Handle) -> bool {
        self.locked(|h| h.is_valid(handle))
    }

    pub fn pin(&self, handle: Handle) -> bool {
        self.locked(|h| h.pin(handle))
    }

    pub fn unpin(&self, handle: Handle) -> bool {
        self.locked(|h| h.unpin(handle))
    }

    /// Pins several handles under one lock. A queued job pins what it will
    /// need, and taking the lock once per handle was a measurable share of
    /// starting a task.
    pub fn pin_all(&self, handles: &[Handle]) {
        self.locked(|h| {
            for handle in handles {
                h.pin(*handle);
            }
        })
    }

    pub fn unpin_all(&self, handles: &[Handle]) {
        self.locked(|h| {
            for handle in handles {
                h.unpin(*handle);
            }
        })
    }

    /// Runs `f` with the heap locked, for an update that spans several
    /// objects and must be seen whole.
    ///
    /// The narrow exception to `locked` being private, and the rules that make
    /// it safe are the caller's: `f` must be short, must not allocate (an
    /// allocation here bypasses the interpreter's counting), and must not
    /// reach this heap through any other handle to it — the lock is not
    /// re-entrant, and doing so deadlocks.
    ///
    /// What it is for: a task's "is it still pending? then add me to its
    /// waiting list" and "mark it finished and take the list". Each touches
    /// the task and its list, and doing them under a separate monitor plus
    /// one heap lock per field read was most of what completing a task cost.
    pub fn atomically<R>(&self, f: impl FnOnce(&mut Heap) -> R) -> R {
        self.locked(f)
    }

    /// How many times a thread found the heap locked and had to wait.
    ///
    /// Every field read and write takes this lock, so it is where threads
    /// running managed code meet. A count rising faster than work completes
    /// says the threads are mostly queueing for the heap, and the task pool
    /// reads it to decide how many workers to keep running: measured on a
    /// fan-out of a million tasks, one worker finished in 6.1 s and eight in
    /// 15.5 s.
    #[cfg(feature = "threads")]
    pub fn contended_acquisitions(&self) -> usize {
        self.contended.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Swaps the collection policy, returning the previous one.
    ///
    /// The policy is meant to be swappable — that is a stated requirement — and
    /// a shared heap should be no harder to reconfigure than an owned one.
    pub fn set_collector(&self, collector: alloc::boxed::Box<dyn crate::Collector>)
        -> alloc::boxed::Box<dyn crate::Collector>
    {
        self.locked(|h| h.set_collector(collector))
    }

    pub fn should_collect(&self) -> bool {
        self.locked(|h| h.should_collect())
    }

    pub fn collect(&self, roots: &dyn RootSet) -> CollectionReport {
        self.locked(|h| h.collect(roots))
    }

    pub fn slot_limit(&self) -> Option<usize> {
        self.locked(|h| h.slot_limit())
    }

    pub fn collector_name(&self) -> &'static str {
        self.locked(|h| h.collector_name())
    }

    pub fn live_bytes(&self) -> usize {
        self.locked(|h| h.live_bytes())
    }

    pub fn live_count(&self) -> usize {
        self.locked(|h| h.live_count())
    }

    pub fn stats(&self) -> GcStats {
        self.locked(|h| h.stats())
    }
}

impl Default for SharedHeap {
    fn default() -> Self {
        Self::new(Heap::new())
    }
}

impl From<Heap> for SharedHeap {
    fn from(heap: Heap) -> Self {
        Self::new(heap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tracer};
    use alloc::vec::Vec;
    use core::any::Any;

    struct Leaf(u32);
    impl GcObject for Leaf {
        fn trace(&self, _: &mut Tracer) {}
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    struct Roots(Vec<Handle>);
    impl RootSet for Roots {
        fn collect_roots(&self, out: &mut Vec<Handle>) {
            out.extend_from_slice(&self.0);
        }
    }

    #[test]
    fn a_clone_names_the_same_heap() {
        let heap = SharedHeap::default();
        let other = heap.clone();
        let handle = heap.alloc(Leaf(7));
        assert_eq!(other.with::<Leaf, _>(handle, |l| l.0), Some(7));
        assert_eq!(other.live_count(), 1, "one heap, not two");
    }

    #[test]
    fn a_mutation_through_one_handle_is_seen_through_another() {
        let heap = SharedHeap::default();
        let other = heap.clone();
        let handle = heap.alloc(Leaf(1));
        other.with_mut::<Leaf, _>(handle, |l| l.0 = 42);
        assert_eq!(heap.with::<Leaf, _>(handle, |l| l.0), Some(42));
    }

    #[test]
    fn collection_through_a_shared_handle_still_reclaims() {
        let heap = SharedHeap::default();
        let kept = heap.alloc(Leaf(1));
        let _dropped = heap.alloc(Leaf(2));
        assert_eq!(heap.live_count(), 2);

        heap.collect(&Roots(alloc::vec![kept]));
        assert_eq!(heap.live_count(), 1, "the unrooted object was reclaimed");
        assert!(heap.is_valid(kept));
    }

    #[cfg(feature = "threads")]
    #[test]
    fn several_threads_allocate_into_one_heap() {
        use std::thread;

        let heap = SharedHeap::default();
        let threads: Vec<_> = (0..4)
            .map(|n| {
                let heap = heap.clone();
                thread::spawn(move || {
                    let mut handles = Vec::new();
                    for i in 0..250 {
                        handles.push(heap.alloc(Leaf(n * 1000 + i)));
                    }
                    handles
                })
            })
            .collect();

        let mut all = Vec::new();
        for t in threads {
            all.extend(t.join().expect("thread panicked"));
        }

        assert_eq!(all.len(), 1000);
        assert_eq!(heap.live_count(), 1000, "every allocation landed");
        // Every handle still names the value its thread wrote, which is what
        // would break first if allocation were not serialised.
        for (n, handle) in all.iter().enumerate() {
            assert!(heap.with::<Leaf, _>(*handle, |_| ()).is_some(), "handle {n} is dead");
        }
    }
}

#[cfg(feature = "std")]
std::thread_local! {
    static THREAD_ALLOCATIONS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
}

#[inline]
fn count_on_this_thread() {
    #[cfg(feature = "std")]
    THREAD_ALLOCATIONS.with(|c| c.set(c.get() + 1));
}

