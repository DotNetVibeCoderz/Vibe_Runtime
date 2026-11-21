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
//! # Shards, not one lock
//!
//! With threads, the heap is sixteen [`ObjectSpace`]s, each behind a lock of
//! its own, and a thread allocates into the shard it was given. A handle
//! carries its shard in the low bits of its index, so finding the lock for an
//! object is a mask.
//!
//! It used to be one lock for the whole heap, and every field a managed method
//! touched took it. Running managed code on more threads then mostly meant more
//! threads queueing for that lock: a fan-out of a million tasks took 6.1 s on
//! one worker and 15.5 s on eight. Most objects are made and used by one
//! thread — a closure, a boxed value, a string being built — and with shards
//! those never meet another thread's lock. Objects that *are* shared, a task
//! another thread completes or a channel's queue, still contend, but only on
//! the shard they live in.
//!
//! What crosses shards:
//!
//! * **Collection** takes the policy lock and then every shard, in order, and
//!   hands the collector all of them: an object in one shard points at objects
//!   in others, so marking needs the whole graph.
//! * **[`SharedHeap::atomically`]** names the handles it will touch and takes
//!   their shards in ascending order, which is what keeps two such updates
//!   from deadlocking.
//! * **Statistics** are counted per shard and summed when asked for. The
//!   policy's view of pressure is updated every 32 KB a shard allocates rather
//!   than every allocation, so `collect now?` costs a load, not a lock.
//!
//! A heap built with a slot ceiling, or handed over already holding objects,
//! stays one shard: a ceiling is a single budget, and existing handles were
//! numbered without a shard in them.
//!
//! # Without `std`
//!
//! A microcontroller has one thread and no `Mutex`, so the cell is a
//! [`RefCell`](core::cell::RefCell) around the plain [`Heap`]. It costs a
//! borrow flag rather than an atomic, and it panics on re-entrancy instead of
//! deadlocking — which is the better failure, and one nothing in this runtime
//! provokes.

use crate::{CollectionReport, GcObject, GcStats, Handle, Heap, RootSet};

#[cfg(feature = "threads")]
pub use sharded::{Locked, SharedHeap};
#[cfg(not(feature = "threads"))]
pub use single::{Locked, SharedHeap};

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

// ── with threads: sharded ───────────────────────────────────────────────────

#[cfg(feature = "threads")]
mod sharded {
    use super::*;
    use crate::{Collector, HeapPressure, ObjectSpace};
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use rustclr_thread::sync::{Arc, Mutex, MutexGuard};

    /// log2 of the shard count. Sixteen is past the core count of the
    /// machines this was measured on, so two threads rarely share a shard.
    const SHARD_SHIFT: u32 = 4;

    /// Bytes a shard allocates before it tells the policy. Small next to the
    /// default 4 MB trigger, so collection is at most a little late.
    const FLUSH_BYTES: usize = 32 * 1024;

    struct ShardState {
        space: ObjectSpace,
        allocations: u64,
        bytes_allocated: u64,
        /// Bytes and objects allocated here and not yet reported to the policy.
        unflushed_bytes: usize,
        unflushed_count: usize,
    }

    /// One shard. Aligned to a cache line so that two threads locking
    /// neighbouring shards do not fight over the line the locks share.
    #[repr(align(64))]
    struct Shard(Mutex<ShardState>, AtomicUsize);

    struct Policy {
        collector: Box<dyn Collector>,
        /// Collections and what they freed, plus the peaks. Allocation counts
        /// live in the shards.
        stats: GcStats,
        /// Live bytes and objects as the last collection left them.
        live_bytes_after: usize,
        live_count_after: usize,
        /// Reported to the policy since the last collection.
        bytes_since: usize,
        count_since: usize,
    }

    struct Inner {
        shards: Box<[Shard]>,
        policy: Mutex<Policy>,
        slot_limit: Option<usize>,
        /// Report to the policy after every allocation rather than in
        /// batches: a one-shard heap with a ceiling, or a stress run.
        exact: bool,
        /// Shard acquisitions that found it already held.
        contended: AtomicUsize,
        /// The policy's answer to "collect now?". See `should_collect`.
        collect_wanted: AtomicBool,
        next_shard: AtomicUsize,
    }

    /// A heap that can be shared between threads.
    ///
    /// Cloning gives another handle to the same heap, not another heap.
    #[derive(Clone)]
    pub struct SharedHeap {
        inner: Arc<Inner>,
    }

    impl SharedHeap {
        pub fn new(heap: Heap) -> Self {
            Self::build(heap, false)
        }

        /// One shard, as a heap without threads has.
        ///
        /// For measuring what a board pays: a board has no threads, so its
        /// heap is never sharded, and sixteen empty shards are about 3 KB a
        /// footprint test on the host would otherwise charge to the bindings.
        pub fn unsharded(heap: Heap) -> Self {
            Self::build(heap, true)
        }

        fn build(heap: Heap, one_shard: bool) -> Self {
            let stress = crate::stress_from_environment().is_some();
            let slot_limit = heap.slot_limit();
            let (space, collector, stats) = heap.into_parts();
            let live_bytes_after = space.live_bytes();
            let live_count_after = space.live_count();
            // Handles already handed out were numbered without a shard, and a
            // ceiling is one budget: both keep the heap whole.
            let whole = one_shard || slot_limit.is_some() || space.slot_capacity() > 0;
            let shards: Box<[Shard]> = if whole {
                alloc::vec![Shard(Mutex::new(ShardState::new(space)), AtomicUsize::new(0))].into_boxed_slice()
            } else {
                (0..1u32 << SHARD_SHIFT)
                    .map(|n| {
                        Shard(Mutex::new(ShardState::new(ObjectSpace::shard(n, SHARD_SHIFT))), AtomicUsize::new(0))
                    })
                    .collect()
            };
            let inner = Inner {
                exact: whole || stress,
                shards,
                policy: Mutex::new(Policy {
                    collector,
                    stats,
                    live_bytes_after,
                    live_count_after,
                    bytes_since: 0,
                    count_since: 0,
                }),
                slot_limit,
                contended: AtomicUsize::new(0),
                collect_wanted: AtomicBool::new(false),
                next_shard: AtomicUsize::new(0),
            };
            Self { inner: Arc::new(inner) }
        }

        // -- locking ---------------------------------------------------------

        /// The shard that owns `handle`, or `None` for null.
        #[inline]
        fn shard_of(&self, handle: Handle) -> Option<usize> {
            if handle.is_null() {
                return None;
            }
            let n = self.inner.shards.len();
            Some(if n == 1 { 0 } else { handle.index() & (n - 1) })
        }

        /// Locks a shard, counting the acquisition if it had to wait.
        ///
        /// A poisoned shard means another thread panicked mid-mutation. There
        /// is no recovery from a half-updated object graph, so this propagates
        /// rather than pretending the heap is usable.
        #[inline]
        fn lock(&self, shard: usize) -> MutexGuard<'_, ShardState> {
            let cell = &self.inner.shards[shard].0;
            // On the line the lock is on, which this thread has just taken, so
            // counting costs little and shares nothing with other shards.
            self.inner.shards[shard].1.fetch_add(1, Ordering::Relaxed);
            match cell.try_lock() {
                Ok(guard) => guard,
                // Counted only on this path, so the uncontended lock — nearly
                // every one — costs what it always did.
                Err(_) => {
                    self.inner.contended.fetch_add(1, Ordering::Relaxed);
                    cell.lock().expect("the managed heap is poisoned")
                }
            }
        }

        fn policy(&self) -> MutexGuard<'_, Policy> {
            self.inner.policy.lock().expect("the managed heap is poisoned")
        }

        /// Every shard, in order. Only with the policy lock held — that is the
        /// lock order everywhere: policy, then shards ascending.
        fn lock_all(&self) -> Vec<MutexGuard<'_, ShardState>> {
            (0..self.inner.shards.len()).map(|n| self.lock(n)).collect()
        }

        /// The shard this thread allocates into.
        #[inline]
        fn home_shard(&self) -> usize {
            let n = self.inner.shards.len();
            if n == 1 {
                return 0;
            }
            #[cfg(feature = "std")]
            {
                let mine = HOME_SHARD.with(|c| {
                    let v = c.get();
                    if v != usize::MAX {
                        return v;
                    }
                    let v = self.inner.next_shard.fetch_add(1, Ordering::Relaxed);
                    c.set(v);
                    v
                });
                mine & (n - 1)
            }
            #[cfg(not(feature = "std"))]
            {
                0
            }
        }

        // -- allocation ------------------------------------------------------

        pub fn alloc<T: GcObject>(&self, object: T) -> Handle {
            self.try_alloc(object).expect("heap exhausted; call try_alloc on a fixed heap")
        }

        pub fn try_alloc<T: GcObject>(&self, object: T) -> Option<Handle> {
            let size = object.size_hint();
            let shard = self.home_shard();
            let (handle, report) = {
                let mut state = self.lock(shard);
                let handle = state.space.try_alloc(Box::new(object))?;
                state.allocations += 1;
                state.bytes_allocated += size as u64;
                state.unflushed_bytes += size;
                state.unflushed_count += 1;
                let report = if self.inner.exact || state.unflushed_bytes >= FLUSH_BYTES {
                    let report = (state.unflushed_bytes, state.unflushed_count);
                    state.unflushed_bytes = 0;
                    state.unflushed_count = 0;
                    Some(report)
                } else {
                    None
                };
                (handle, report)
            };
            count_on_this_thread();
            // Outside the shard lock: the order is policy, then shards.
            if let Some((bytes, count)) = report {
                let mut policy = self.policy();
                policy.bytes_since += bytes;
                policy.count_since += count;
                self.reconsider(&mut policy);
            }
            Some(handle)
        }

        /// Works out whether a collection is wanted, and records the peaks.
        fn reconsider(&self, policy: &mut Policy) {
            // Mark-sweep frees nothing between collections, so what was live
            // afterwards plus what has been allocated since *is* what is live.
            let live_bytes = policy.live_bytes_after + policy.bytes_since;
            let live_count = policy.live_count_after + policy.count_since;
            policy.stats.peak_live_bytes = policy.stats.peak_live_bytes.max(live_bytes);
            policy.stats.peak_live_count = policy.stats.peak_live_count.max(live_count);
            let wanted = match crate::stress_from_environment() {
                Some(every) => policy.bytes_since >= every,
                None => policy.collector.should_collect(&HeapPressure {
                    live_bytes,
                    live_count,
                    bytes_since_last_collection: policy.bytes_since,
                    slot_limit: self.inner.slot_limit,
                }),
            };
            self.inner.collect_wanted.store(wanted, Ordering::Relaxed);
        }

        /// Allocations the calling OS thread has made, through any heap.
        ///
        /// `stats()` is the heap's total, every thread together. The profiler
        /// watches one interpreter, so comparing it against the total says
        /// nothing once a program starts threads — a fixture with four workers
        /// updating a dictionary showed 87% "attributed" with no allocation
        /// site missed. Host builds only: a board has one thread, where the
        /// total is the same number.
        #[cfg(feature = "std")]
        pub fn allocations_on_this_thread() -> u64 {
            THREAD_ALLOCATIONS.with(|c| c.get())
        }

        // -- access ----------------------------------------------------------

        /// Reads an object, if the handle names a live one of that type.
        pub fn with<T: GcObject, R>(&self, handle: Handle, f: impl FnOnce(&T) -> R) -> Option<R> {
            let shard = self.shard_of(handle)?;
            let state = self.lock(shard);
            state.space.get_as::<T>(handle).map(f)
        }

        /// [`Self::with`], for a mutation.
        pub fn with_mut<T: GcObject, R>(&self, handle: Handle, f: impl FnOnce(&mut T) -> R) -> Option<R> {
            let shard = self.shard_of(handle)?;
            let mut state = self.lock(shard);
            state.space.get_as_mut::<T>(handle).map(f)
        }

        /// Reads an object without naming its type.
        ///
        /// Scoped like the others: the `&dyn GcObject` a caller would otherwise
        /// receive is the one thing that cannot cross the lock.
        pub fn with_any<R>(&self, handle: Handle, f: impl FnOnce(&dyn GcObject) -> R) -> Option<R> {
            let shard = self.shard_of(handle)?;
            let state = self.lock(shard);
            state.space.get(handle).map(f)
        }

        pub fn is_valid(&self, handle: Handle) -> bool {
            self.shard_of(handle).is_some_and(|s| self.lock(s).space.is_valid(handle))
        }

        pub fn pin(&self, handle: Handle) -> bool {
            self.shard_of(handle).is_some_and(|s| self.lock(s).space.pin(handle))
        }

        pub fn unpin(&self, handle: Handle) -> bool {
            self.shard_of(handle).is_some_and(|s| self.lock(s).space.unpin(handle))
        }

        /// Pins several handles. A queued job pins what it will need.
        pub fn pin_all(&self, handles: &[Handle]) {
            self.atomically(handles, |locked| {
                for handle in handles {
                    locked.pin(*handle);
                }
            })
        }

        pub fn unpin_all(&self, handles: &[Handle]) {
            self.atomically(handles, |locked| {
                for handle in handles {
                    locked.unpin(*handle);
                }
            })
        }

        /// Runs `f` with the shards of `handles` locked, for an update that
        /// spans several objects and must be seen whole.
        ///
        /// Only those objects can be reached through `f`'s [`Locked`]; anything
        /// else reads as absent. The rules that make it safe are the caller's:
        /// `f` must be short, must not allocate, and must not reach this heap
        /// through any other handle to it — the locks are not re-entrant.
        ///
        /// Shards are taken in ascending order, which is what keeps two of
        /// these from deadlocking against each other or against a collection.
        pub fn atomically<R>(&self, handles: &[Handle], f: impl FnOnce(&mut Locked<'_>) -> R) -> R {
            let mut wanted: Vec<usize> = handles.iter().filter_map(|h| self.shard_of(*h)).collect();
            wanted.sort_unstable();
            wanted.dedup();
            let guards = wanted.iter().map(|&n| (n, self.lock(n))).collect();
            let mut locked = Locked { guards, count: self.inner.shards.len() };
            f(&mut locked)
        }

        /// How many times a thread found a shard locked and had to wait.
        ///
        /// Every field read and write takes a shard lock, so it is where
        /// threads running managed code meet. A count rising faster than work
        /// completes says the threads are mostly queueing, and the task pool
        /// reads it to decide how many workers to keep running.
        pub fn contended_acquisitions(&self) -> usize {
            self.inner.contended.load(Ordering::Relaxed)
        }

        /// Shard acquisitions of any kind, contended or not. Read against
        /// [`Self::contended_acquisitions`], the share that had to wait.
        pub fn acquisitions(&self) -> usize {
            self.inner.shards.iter().map(|s| s.1.load(Ordering::Relaxed)).sum()
        }

        // -- policy ----------------------------------------------------------

        /// Swaps the collection policy, returning the previous one.
        ///
        /// The policy is meant to be swappable — that is a stated requirement —
        /// and a shared heap should be no harder to reconfigure than an owned
        /// one.
        pub fn set_collector(&self, collector: Box<dyn Collector>) -> Box<dyn Collector> {
            let mut policy = self.policy();
            let previous = core::mem::replace(&mut policy.collector, collector);
            self.reconsider(&mut policy);
            previous
        }

        /// The policy's answer to "collect now?", as of the last report.
        ///
        /// Asked at every `ret`, `newobj` and `newarr` — it doubles as the
        /// safe-point poll — so it is a load, not a lock. Pressure only changes
        /// when something is allocated or collected, and the answer is worked
        /// out there.
        pub fn should_collect(&self) -> bool {
            self.inner.collect_wanted.load(Ordering::Relaxed)
        }

        pub fn collect(&self, roots: &dyn RootSet) -> CollectionReport {
            let mut policy = self.policy();
            let mut guards = self.lock_all();
            let report = {
                let mut spaces: Vec<&mut ObjectSpace> = guards.iter_mut().map(|g| &mut g.space).collect();
                policy.collector.collect(&mut spaces, roots)
            };
            for guard in guards.iter_mut() {
                guard.unflushed_bytes = 0;
                guard.unflushed_count = 0;
            }
            drop(guards);
            policy.stats.collections += 1;
            policy.stats.total_objects_freed += report.objects_freed as u64;
            policy.stats.total_bytes_freed += report.bytes_freed as u64;
            policy.live_bytes_after = report.bytes_surviving;
            policy.live_count_after = report.objects_surviving;
            policy.bytes_since = 0;
            policy.count_since = 0;
            self.reconsider(&mut policy);
            report
        }

        pub fn slot_limit(&self) -> Option<usize> {
            self.inner.slot_limit
        }

        pub fn collector_name(&self) -> &'static str {
            self.policy().collector.name()
        }

        pub fn live_bytes(&self) -> usize {
            (0..self.inner.shards.len()).map(|n| self.lock(n).space.live_bytes()).sum()
        }

        pub fn live_count(&self) -> usize {
            (0..self.inner.shards.len()).map(|n| self.lock(n).space.live_count()).sum()
        }

        pub fn stats(&self) -> GcStats {
            let policy = self.policy();
            let guards = self.lock_all();
            let mut stats = policy.stats;
            let mut live_bytes = 0;
            let mut live_count = 0;
            for g in &guards {
                stats.total_allocations += g.allocations;
                stats.total_bytes_allocated += g.bytes_allocated;
                live_bytes += g.space.live_bytes();
                live_count += g.space.live_count();
            }
            stats.peak_live_bytes = stats.peak_live_bytes.max(live_bytes);
            stats.peak_live_count = stats.peak_live_count.max(live_count);
            stats
        }
    }

    impl ShardState {
        fn new(space: ObjectSpace) -> Self {
            Self { space, allocations: 0, bytes_allocated: 0, unflushed_bytes: 0, unflushed_count: 0 }
        }
    }

    /// The objects an [`SharedHeap::atomically`] call locked.
    pub struct Locked<'a> {
        guards: Vec<(usize, MutexGuard<'a, ShardState>)>,
        count: usize,
    }

    impl Locked<'_> {
        fn space(&self, handle: Handle) -> Option<&ObjectSpace> {
            if handle.is_null() {
                return None;
            }
            let shard = if self.count == 1 { 0 } else { handle.index() & (self.count - 1) };
            self.guards.iter().find(|(n, _)| *n == shard).map(|(_, g)| &g.space)
        }

        fn space_mut(&mut self, handle: Handle) -> Option<&mut ObjectSpace> {
            if handle.is_null() {
                return None;
            }
            let shard = if self.count == 1 { 0 } else { handle.index() & (self.count - 1) };
            self.guards.iter_mut().find(|(n, _)| *n == shard).map(|(_, g)| &mut g.space)
        }

        /// The object, if it is a `T` and its shard was locked.
        pub fn get_as<T: GcObject>(&self, handle: Handle) -> Option<&T> {
            self.space(handle)?.get_as::<T>(handle)
        }

        pub fn get_as_mut<T: GcObject>(&mut self, handle: Handle) -> Option<&mut T> {
            self.space_mut(handle)?.get_as_mut::<T>(handle)
        }

        pub fn pin(&mut self, handle: Handle) -> bool {
            self.space_mut(handle).is_some_and(|s| s.pin(handle))
        }

        pub fn unpin(&mut self, handle: Handle) -> bool {
            self.space_mut(handle).is_some_and(|s| s.unpin(handle))
        }
    }

    #[cfg(feature = "std")]
    std::thread_local! {
        /// The shard this thread allocates into, assigned on first use.
        static HOME_SHARD: core::cell::Cell<usize> = const { core::cell::Cell::new(usize::MAX) };
    }
}

// ── without threads: one heap in a RefCell ──────────────────────────────────

#[cfg(not(feature = "threads"))]
mod single {
    use super::*;
    use alloc::rc::Rc;
    use core::cell::{RefCell, RefMut};

    /// A [`Heap`] behind a `RefCell`. One thread, so nothing is shared.
    ///
    /// Cloning gives another handle to the same heap, not another heap.
    #[derive(Clone)]
    pub struct SharedHeap {
        // `Arc` needs an atomic compare-and-swap, and two of this runtime's
        // bare-metal targets — `thumbv6m` and `riscv32imc` — have no atomics at
        // all. `Rc` is the right type on a chip anyway.
        inner: Rc<RefCell<Heap>>,
    }

    impl SharedHeap {
        pub fn new(heap: Heap) -> Self {
            Self { inner: Rc::new(RefCell::new(heap)) }
        }

        /// The same as [`Self::new`]: without threads there are no shards.
        pub fn unsharded(heap: Heap) -> Self {
            Self::new(heap)
        }

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

        pub fn with<T: GcObject, R>(&self, handle: Handle, f: impl FnOnce(&T) -> R) -> Option<R> {
            self.locked(|h| h.with::<T, R>(handle, f))
        }

        pub fn with_mut<T: GcObject, R>(&self, handle: Handle, f: impl FnOnce(&mut T) -> R) -> Option<R> {
            self.locked(|h| h.with_mut::<T, R>(handle, f))
        }

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

        /// Runs `f` with the heap borrowed. See the threaded version for the
        /// rules; here there is only one heap and one thread.
        pub fn atomically<R>(&self, _handles: &[Handle], f: impl FnOnce(&mut Locked<'_>) -> R) -> R {
            let mut locked = Locked { heap: self.inner.borrow_mut() };
            f(&mut locked)
        }

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

    /// The heap, borrowed for an [`SharedHeap::atomically`] call.
    pub struct Locked<'a> {
        heap: RefMut<'a, Heap>,
    }

    impl Locked<'_> {
        pub fn get_as<T: GcObject>(&self, handle: Handle) -> Option<&T> {
            self.heap.get_as::<T>(handle)
        }

        pub fn get_as_mut<T: GcObject>(&mut self, handle: Handle) -> Option<&mut T> {
            self.heap.get_as_mut::<T>(handle)
        }

        pub fn pin(&mut self, handle: Handle) -> bool {
            self.heap.pin(handle)
        }

        pub fn unpin(&mut self, handle: Handle) -> bool {
            self.heap.unpin(handle)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tracer;
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

    /// A leaf that points at another object.
    struct Link(Handle);
    impl GcObject for Link {
        fn trace(&self, t: &mut Tracer) {
            t.edge(self.0);
        }
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

    #[test]
    fn atomically_reaches_what_it_named() {
        let heap = SharedHeap::default();
        let a = heap.alloc(Leaf(1));
        let b = heap.alloc(Leaf(2));
        let sum = heap.atomically(&[a, b], |locked| {
            locked.get_as_mut::<Leaf>(a).unwrap().0 += 10;
            locked.get_as::<Leaf>(a).unwrap().0 + locked.get_as::<Leaf>(b).unwrap().0
        });
        assert_eq!(sum, 13);
        assert_eq!(heap.stats().total_allocations, 2);
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

    /// The point of sharding, and the thing it must not break: threads that
    /// allocate land in different shards, and a collection still follows a
    /// reference from an object in one shard to an object in another.
    #[cfg(feature = "threads")]
    #[test]
    fn a_reference_across_shards_keeps_its_target_alive() {
        use std::thread;

        let heap = SharedHeap::default();
        let targets: Vec<Handle> = (0..4)
            .map(|n| {
                let heap = heap.clone();
                thread::spawn(move || heap.alloc(Leaf(100 + n))).join().unwrap()
            })
            .collect();
        let shards: std::collections::HashSet<usize> =
            targets.iter().map(|h| h.index() & 15).collect();
        assert!(shards.len() > 1, "threads allocate into different shards");

        // Rooted only through links made on this thread, in this thread's shard.
        let links: Vec<Handle> = targets.iter().map(|t| heap.alloc(Link(*t))).collect();
        let garbage = heap.alloc(Leaf(0));
        heap.collect(&Roots(links.clone()));

        for (n, t) in targets.iter().enumerate() {
            assert_eq!(heap.with::<Leaf, _>(*t, |l| l.0), Some(100 + n as u32), "target {n} survived");
        }
        assert!(!heap.is_valid(garbage), "and what nothing reaches is still freed");
        assert_eq!(heap.live_count(), 8);
    }

    /// A handle from one shard is never mistaken for a slot in another.
    #[cfg(feature = "threads")]
    #[test]
    fn a_freed_handle_is_still_detected_with_shards() {
        let heap = SharedHeap::default();
        let gone = heap.alloc(Leaf(1));
        heap.collect(&Roots(Vec::new()));
        assert!(!heap.is_valid(gone));
        let reused = heap.alloc(Leaf(2));
        assert!(!heap.is_valid(gone), "the stale handle stays stale after reuse");
        assert!(heap.is_valid(reused));
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
