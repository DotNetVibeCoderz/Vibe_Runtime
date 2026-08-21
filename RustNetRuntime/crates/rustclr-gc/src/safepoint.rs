//! Stopping every mutator thread so the collector can run.
//!
//! A collection has to see every root. With one thread that is trivial — the
//! runtime hands its frames to [`Heap::collect`] and nothing else can be
//! holding a handle. With several, a thread that starts collecting must first
//! bring the others to a stop, because a root sitting in another thread's frame
//! is invisible from here and the object under it would be reclaimed while that
//! thread is still using it.
//!
//! This is the handshake. It is deliberately the *first* thing built for
//! multi-threaded execution, before any of the mechanical work of sharing the
//! heap: if this protocol does not hold up, nothing built on top of it would
//! either.
//!
//! # The protocol
//!
//! Every mutator registers once and then polls at safe points — places where it
//! holds no reference into the heap and its roots are accurately described by
//! its frames. The interpreter already has exactly one such place, where it
//! calls `maybe_collect`.
//!
//! ```text
//! collector thread                     other mutators
//! ----------------                     --------------
//! request()        ─── sets a flag ──▶ poll() sees it
//!                                      publishes its roots
//!                                      parks
//! wait for all parked ◀────────────────
//! collect with every root
//! release()        ─── clears flag ──▶ mutators wake and continue
//! ```
//!
//! # What makes it safe
//!
//! A parked mutator holds no borrow into the heap, because a safe point is
//! defined as a place where it holds none. So the collector may take the heap
//! lock knowing no other thread is inside it.
//!
//! # What it is not
//!
//! It does not make the heap shareable — that is the mechanical half, and it is
//! larger. This module is the part that had to be proven first.
//!
//! # Without `std`
//!
//! A microcontroller has one thread, so there is nothing to stop and no
//! `Mutex` or `Condvar` to stop it with. The shim at the bottom of this file
//! keeps the same API and does nothing: `poll` returns, and `stop_the_world`
//! collects against the caller's own roots. The interpreter therefore has one
//! collection path on every target rather than a `cfg` at each call site, and
//! the shim compiles away.

#[cfg(feature = "std")]
use std::sync::{Arc, Condvar, Mutex};

use crate::handle::Handle;
use crate::RootSet;

#[cfg(feature = "std")]
mod std_impl {
    use super::*;
/// Roots gathered from every mutator, as one root set.
///
/// The collector does not care which thread a root came from, only that it has
/// them all — so the aggregate is the same shape as a single thread's.
pub struct AllRoots(Vec<Handle>);

impl RootSet for AllRoots {
    fn collect_roots(&self, out: &mut Vec<Handle>) {
        out.extend_from_slice(&self.0);
    }
}

#[derive(Default)]
struct State {
    /// Mutators that have registered and not yet unregistered.
    registered: usize,
    /// Mutators currently parked at a safe point during a collection.
    parked: usize,
    /// Mutators that are blocked outside the runtime — in a native call, or
    /// waiting on something — and therefore already stopped as far as a
    /// collection is concerned.
    blocked: usize,
    /// Roots of threads that are blocked outside the runtime.
    ///
    /// A blocked thread cannot poll, so it hands its roots over on the way in
    /// and they stay here until it comes back. Without this a collection sees
    /// a blocked thread as *stopped* and sweeps everything only its frames
    /// reach — which is most of what a thread sitting in `Thread.Join` is
    /// holding. That bug swept an array out from under a live local.
    blocked_roots: Vec<(u64, Vec<Handle>)>,
    /// Hands out the ids `blocked_roots` is keyed by.
    next_blocked_id: u64,
    /// Set while a collection is requested or running.
    stopping: bool,
    /// Roots published by parked mutators, plus the collector's own.
    roots: Vec<Handle>,
    /// Bumped every time a collection finishes, so a waking mutator can tell
    /// "the collection I parked for is over" from "another has already begun".
    epoch: u64,
}

/// The registry every mutator thread joins.
///
/// Cheap to clone: it is an `Arc` inside, and every clone names the same
/// registry.
#[derive(Clone, Default)]
pub struct Mutators {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    /// Signalled when a mutator parks, so a waiting collector can re-check.
    parked: Condvar,
    /// Signalled when a collection ends, so parked mutators can resume.
    resumed: Condvar,
}

impl Mutators {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the calling thread and returns a guard that unregisters it.
    ///
    /// Registration has to be balanced or a collection waits forever for a
    /// thread that has gone — so it is a guard rather than a pair of calls.
    pub fn register(&self) -> Registration {
        let mut state = self.inner.state.lock().expect("mutator registry poisoned");
        state.registered += 1;
        drop(state);
        Registration { mutators: self.clone() }
    }

    /// Number of registered mutators, for tests and diagnostics.
    pub fn registered(&self) -> usize {
        self.inner.state.lock().map(|s| s.registered).unwrap_or(0)
    }

    /// Whether a collection is being requested right now.
    ///
    /// A mutator's fast path: a load under a lock, and no work when the answer
    /// is no.
    pub fn stopping(&self) -> bool {
        self.inner.state.lock().map(|s| s.stopping).unwrap_or(false)
    }

    /// A safe point.
    ///
    /// Does nothing unless a collection has been requested. When one has, it
    /// publishes this thread's roots and parks until the collection is over.
    /// The closure is only called when the roots are actually needed, so a
    /// mutator pays nothing for polling in the common case.
    pub fn poll(&self, roots: impl FnOnce() -> Vec<Handle>) {
        let mut state = self.inner.state.lock().expect("mutator registry poisoned");
        if !state.stopping {
            return;
        }
        let waiting_for = state.epoch;
        state.roots.extend(roots());
        state.parked += 1;
        self.inner.parked.notify_all();

        // A loop, not a single wait: condition variables may wake spuriously,
        // and the epoch distinguishes "my collection finished" from "another
        // one started before I woke".
        while state.stopping && state.epoch == waiting_for {
            state = self.inner.resumed.wait(state).expect("mutator registry poisoned");
        }
        state.parked -= 1;
        // Announce the departure. A collector waiting for the previous round to
        // drain is watching this counter reach zero.
        self.inner.parked.notify_all();
    }

    /// Stops every other mutator, runs `collect`, then releases them.
    ///
    /// `own_roots` are the calling thread's; it does not park itself.
    ///
    /// The closure receives every root in the process. It runs with the
    /// registry locked, which is what keeps a mutator from leaving its safe
    /// point while the collector is working.
    pub fn stop_the_world<R>(
        &self,
        own_roots: Vec<Handle>,
        collect: impl FnOnce(&AllRoots) -> R,
    ) -> R {
        let mut state = self.inner.state.lock().expect("mutator registry poisoned");

        // Two things have to be true before this round can start.
        //
        // One collection at a time: a thread that arrives while another is
        // stopping the world parks like any other mutator and tries again.
        //
        // And no mutator may still be parked from an *earlier* round. That
        // second condition is not obvious and its absence is a live bug: a
        // thread woken by the previous collection but not yet rescheduled is
        // still counted in `parked`, so a collector starting immediately
        // afterwards mistakes it for a thread that has arrived at *this* safe
        // point and sweeps without its roots. Back-to-back collections made it
        // reproducible; a single collection never shows it.
        while state.stopping || state.parked > 0 {
            if state.stopping {
                let waiting_for = state.epoch;
                state.parked += 1;
                self.inner.parked.notify_all();
                while state.stopping && state.epoch == waiting_for {
                    state = self.inner.resumed.wait(state).expect("mutator registry poisoned");
                }
                state.parked -= 1;
                self.inner.parked.notify_all();
            } else {
                state = self.inner.parked.wait(state).expect("mutator registry poisoned");
            }
        }

        state.stopping = true;
        state.roots.clear();
        state.roots.extend(own_roots);
        // A thread away in a blocking call cannot hand its roots over now, so
        // it left them here on the way out.
        let blocked: Vec<Handle> =
            state.blocked_roots.iter().flat_map(|(_, r)| r.iter().copied()).collect();
        state.roots.extend(blocked);

        // Every registered mutator except this one has to be stopped — either
        // parked at a safe point, or blocked outside the runtime, which amounts
        // to the same thing: it holds no reference into the heap.
        let others = state.registered.saturating_sub(1);
        while state.parked + state.blocked < others {
            state = self.inner.parked.wait(state).expect("mutator registry poisoned");
        }

        let all = AllRoots(core::mem::take(&mut state.roots));
        let result = collect(&all);

        state.stopping = false;
        state.epoch = state.epoch.wrapping_add(1);
        self.inner.resumed.notify_all();
        result
    }

    /// Marks this thread as blocked outside the runtime until the guard drops.
    ///
    /// A registered thread that stops polling wedges every collection: the
    /// collector waits for it to reach a safe point and it never does. That is
    /// not hypothetical — the first version of this protocol deadlocked exactly
    /// so, with the main thread registered and then sitting in `join`.
    ///
    /// A thread that is blocked holds no reference into the heap, which is the
    /// same property a safe point has. So it counts as stopped, and a
    /// collection can proceed without it.
    ///
    /// Use it around anything that waits: a blocking native call, a join, a
    /// read from a socket.
    pub fn blocked(&self) -> Blocked {
        self.blocked_with(Vec::new())
    }

    /// [`Self::blocked`], for a thread that still holds live references.
    ///
    /// The roots are published for as long as the thread is away. Entering is
    /// a safe point — the thread is about to stop touching the heap — so they
    /// are accurate at the moment they are taken and cannot change until it
    /// returns.
    pub fn blocked_with(&self, roots: Vec<Handle>) -> Blocked {
        let mut state = self.inner.state.lock().expect("mutator registry poisoned");
        state.blocked += 1;
        state.next_blocked_id = state.next_blocked_id.wrapping_add(1).max(1);
        let id = state.next_blocked_id;
        state.blocked_roots.push((id, roots));
        // A collector may already be waiting for this thread. It will not park
        // now, so wake the collector to re-check against the blocked count.
        self.inner.parked.notify_all();
        drop(state);
        Blocked { mutators: self.clone(), id }
    }
}

/// A thread that is blocked outside the runtime, and so counts as stopped.
pub struct Blocked {
    mutators: Mutators,
    /// Which entry of `blocked_roots` belongs to this guard.
    id: u64,
}

impl Drop for Blocked {
    fn drop(&mut self) {
        let inner = &self.mutators.inner;
        let mut state = inner.state.lock().expect("mutator registry poisoned");
        state.blocked -= 1;
        state.blocked_roots.retain(|(id, _)| *id != self.id);

        // Coming back is itself a safe point. If a collection is under way the
        // thread must not walk back into the heap, so it parks like any other
        // mutator until the collection finishes.
        if state.stopping {
            let waiting_for = state.epoch;
            state.parked += 1;
            inner.parked.notify_all();
            while state.stopping && state.epoch == waiting_for {
                state = inner.resumed.wait(state).expect("mutator registry poisoned");
            }
            state.parked -= 1;
        }
    }
}

/// Unregisters a mutator when it goes out of scope.
pub struct Registration {
    mutators: Mutators,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut state = self.mutators.inner.state.lock().expect("mutator registry poisoned");
        state.registered = state.registered.saturating_sub(1);
        // A collector may be waiting for this thread to park. It never will
        // now, so wake it to re-check against the smaller count.
        self.mutators.inner.parked.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn a_lone_mutator_collects_without_waiting() {
        let mutators = Mutators::new();
        let _me = mutators.register();
        let seen = mutators.stop_the_world(vec![Handle::NULL], |roots| {
            let mut out = Vec::new();
            roots.collect_roots(&mut out);
            out.len()
        });
        assert_eq!(seen, 1, "its own roots and no one else's");
    }

    #[test]
    fn every_mutator_is_stopped_and_its_roots_are_seen() {
        let mutators = Mutators::new();
        let _me = mutators.register();

        let running = Arc::new(AtomicBool::new(true));
        let polls = Arc::new(AtomicUsize::new(0));

        // Two other threads, each polling with a root of its own.
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let mutators = mutators.clone();
                let running = running.clone();
                let polls = polls.clone();
                thread::spawn(move || {
                    let _registration = mutators.register();
                    while running.load(Ordering::Relaxed) {
                        mutators.poll(|| {
                            polls.fetch_add(1, Ordering::Relaxed);
                            vec![Handle::NULL]
                        });
                        thread::yield_now();
                    }
                })
            })
            .collect();

        // Wait for both to register, or the collector would not wait for them.
        while mutators.registered() < 3 {
            thread::yield_now();
        }

        let total = mutators.stop_the_world(vec![Handle::NULL], |roots| {
            let mut out = Vec::new();
            roots.collect_roots(&mut out);
            out.len()
        });

        running.store(false, Ordering::Relaxed);
        for w in workers {
            w.join().expect("worker panicked");
        }

        assert_eq!(total, 3, "one root from this thread and one from each worker");
        assert!(polls.load(Ordering::Relaxed) >= 2, "both workers reached a safe point");
    }

    #[test]
    fn collections_from_several_threads_do_not_overlap() {
        let mutators = Mutators::new();
        let _me = mutators.register();

        let inside = Arc::new(AtomicUsize::new(0));
        let overlapped = Arc::new(AtomicBool::new(false));

        let threads: Vec<_> = (0..4)
            .map(|_| {
                let mutators = mutators.clone();
                let inside = inside.clone();
                let overlapped = overlapped.clone();
                thread::spawn(move || {
                    let _registration = mutators.register();
                    for _ in 0..20 {
                        mutators.stop_the_world(vec![Handle::NULL], |_| {
                            // Two collectors here at once would be the bug this
                            // whole protocol exists to prevent.
                            if inside.fetch_add(1, Ordering::SeqCst) != 0 {
                                overlapped.store(true, Ordering::SeqCst);
                            }
                            thread::sleep(Duration::from_micros(50));
                            inside.fetch_sub(1, Ordering::SeqCst);
                        });
                        mutators.poll(Vec::new);
                    }
                })
            })
            .collect();

        // This thread is registered and is about to stop polling, so it has to
        // say so. Without this the test deadlocks — which is how the need for
        // `blocked` was found in the first place.
        let waiting = mutators.blocked();
        for t in threads {
            t.join().expect("collector thread panicked");
        }
        drop(waiting);

        assert!(!overlapped.load(Ordering::SeqCst), "two collections ran at once");
    }

    #[test]
    fn a_blocked_mutator_does_not_wedge_a_collection() {
        // The deadlock this protocol had at first: a registered thread that
        // stops polling is one the collector waits for forever. Declaring it
        // blocked is what makes the collection possible — and the timeout here
        // is the assertion, since the failure mode is a hang rather than a
        // wrong answer.
        let mutators = Mutators::new();
        let _me = mutators.register();

        let sleeping = {
            let mutators = mutators.clone();
            thread::spawn(move || {
                let _registration = mutators.register();
                let _blocked = mutators.blocked();
                thread::sleep(Duration::from_millis(300));
            })
        };

        while mutators.registered() < 2 {
            thread::yield_now();
        }

        let done = Arc::new(AtomicBool::new(false));
        {
            let done = done.clone();
            let mutators = mutators.clone();
            thread::spawn(move || {
                mutators.stop_the_world(vec![Handle::NULL], |_| {});
                done.store(true, Ordering::SeqCst);
            })
            .join()
            .expect("collector panicked");
        }

        assert!(done.load(Ordering::SeqCst), "the collection completed despite a blocked thread");
        sleeping.join().expect("thread panicked");
    }

    #[test]
    fn a_mutator_that_leaves_does_not_wedge_a_collection() {
        let mutators = Mutators::new();
        let _me = mutators.register();

        // Registers and exits without ever polling. A collector that waited for
        // it to park would wait forever, which is why `Registration` wakes the
        // collector on the way out.
        let leaver = {
            let mutators = mutators.clone();
            thread::spawn(move || {
                let _registration = mutators.register();
            })
        };
        leaver.join().expect("thread panicked");

        let seen = mutators.stop_the_world(vec![Handle::NULL], |roots| {
            let mut out = Vec::new();
            roots.collect_roots(&mut out);
            out.len()
        });
        assert_eq!(seen, 1);
    }

    /// A thread away in a blocking call still owns everything its frames hold.
    ///
    /// This is the bug that swept a live array: `blocked` counted the thread as
    /// stopped, which is right, and contributed none of its roots, which is
    /// not. A thread sitting in `Thread.Join` is holding the list of threads it
    /// is joining.
    #[test]
    fn a_blocked_thread_still_owns_its_roots() {
        // One registered mutator: this thread. A second registration would be a
        // thread that never polls, and the collector would wait for it forever
        // — which is what this test did on its first outing.
        let mutators = Mutators::default();
        let _mine = mutators.register();

        let kept = Handle::from_bits(0x0000_0001_0000_0007);
        let guard = mutators.blocked_with(alloc::vec![kept]);

        let seen = mutators.stop_the_world(Vec::new(), |all| {
            let mut out = Vec::new();
            all.collect_roots(&mut out);
            out
        });
        assert!(seen.contains(&kept), "a blocked thread's roots reached the collector");

        drop(guard);
        let after = mutators.stop_the_world(Vec::new(), |all| {
            let mut out = Vec::new();
            all.collect_roots(&mut out);
            out
        });
        assert!(!after.contains(&kept), "and are gone once it returns");
    }
}

}

#[cfg(feature = "std")]
pub use std_impl::*;

// ── without `std` ────────────────────────────────────────────────────────────

/// Roots gathered from every mutator, as one root set.
///
/// On one thread that is just the caller's own roots.
#[cfg(not(feature = "std"))]
pub struct AllRoots(alloc::vec::Vec<Handle>);

#[cfg(not(feature = "std"))]
impl RootSet for AllRoots {
    fn collect_roots(&self, out: &mut alloc::vec::Vec<Handle>) {
        out.extend_from_slice(&self.0);
    }
}

/// The single-threaded shim. Every method is a no-op or a direct call.
#[cfg(not(feature = "std"))]
#[derive(Clone, Default)]
pub struct Mutators;

#[cfg(not(feature = "std"))]
impl Mutators {
    pub fn register(&self) -> Registration {
        Registration
    }

    pub fn registered(&self) -> usize {
        1
    }

    pub fn stopping(&self) -> bool {
        false
    }

    /// Nothing to stop, so nothing to wait for. The roots closure is never
    /// called, which is why it is a closure.
    pub fn poll(&self, _roots: impl FnOnce() -> alloc::vec::Vec<Handle>) {}

    pub fn stop_the_world<R>(
        &self,
        own_roots: alloc::vec::Vec<Handle>,
        collect: impl FnOnce(&AllRoots) -> R,
    ) -> R {
        collect(&AllRoots(own_roots))
    }

    pub fn blocked(&self) -> Blocked {
        Blocked
    }

    /// One thread, so there is nothing to publish roots *to*: the collector is
    /// this thread, and it already has them.
    pub fn blocked_with(&self, _roots: alloc::vec::Vec<Handle>) -> Blocked {
        Blocked
    }

}

#[cfg(not(feature = "std"))]
pub struct Registration;

#[cfg(not(feature = "std"))]
pub struct Blocked;
