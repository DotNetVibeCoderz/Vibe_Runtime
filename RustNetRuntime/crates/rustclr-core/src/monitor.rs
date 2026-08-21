//! `lock (x)`, and the mutual exclusion behind it.
//!
//! Once `Thread.Start` spawns a real OS thread, a `Monitor` that does nothing
//! stops being a harmless simplification and becomes a data race in the user's
//! program. `lock (gate) counter++` on two threads gave 1,863 instead of 2,000
//! the first time threads were real — both threads reached the same static,
//! which was the point, and neither excluded the other, which was not.
//!
//! # Shape
//!
//! One registry per runtime, keyed by the object being locked. Each entry
//! records who owns it and how deep — `lock` is re-entrant in C#, and a method
//! that locks an object it already holds must not deadlock against itself.
//!
//! # Waiting is a safe point
//!
//! A thread blocked on a monitor holds no reference into the heap and cannot
//! reach a poll, so it announces itself blocked for the duration. Without that,
//! a collection would wait for a thread that is itself waiting for a lock the
//! collector's thread holds — which is a deadlock with two participants and no
//! cycle either of them can see.

use crate::prelude::*;

#[cfg(feature = "std")]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(feature = "std")]
use std::thread::ThreadId;

use rustclr_gc::Handle;

#[cfg(feature = "std")]
#[derive(Default)]
struct State {
    /// Owner and recursion depth, by locked object.
    held: HashMap<u64, (ThreadId, usize)>,
}

/// The monitors of one runtime.
#[cfg(feature = "std")]
#[derive(Clone, Default)]
pub struct Monitors {
    inner: Arc<Inner>,
}

#[cfg(feature = "std")]
#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    released: Condvar,
}

#[cfg(feature = "std")]
impl Monitors {
    /// Takes the monitor on `object`, waiting if another thread holds it.
    ///
    /// `blocked` is called around the wait, so the collector counts this thread
    /// as stopped while it cannot poll. It is a closure rather than a guard
    /// because the caller owns the mutator registry and this module does not.
    pub fn enter(&self, object: Handle, blocked: &mut dyn FnMut(&mut dyn FnMut())) {
        let me = std::thread::current().id();
        let key = object.to_bits();

        // The fast path: uncontended, or already ours.
        {
            let mut state = self.lock();
            match state.held.get_mut(&key) {
                None => {
                    state.held.insert(key, (me, 1));
                    return;
                }
                Some((owner, depth)) if *owner == me => {
                    *depth += 1;
                    return;
                }
                Some(_) => {}
            }
        }

        // Contended. Everything from here is done while announced as blocked.
        blocked(&mut || {
            let mut state = self.lock();
            loop {
                match state.held.get_mut(&key) {
                    None => {
                        state.held.insert(key, (me, 1));
                        return;
                    }
                    Some((owner, depth)) if *owner == me => {
                        *depth += 1;
                        return;
                    }
                    Some(_) => {
                        state = self
                            .inner
                            .released
                            .wait(state)
                            .expect("the monitor registry is poisoned");
                    }
                }
            }
        });
    }

    /// Releases one level of the monitor on `object`.
    ///
    /// Exiting a monitor this thread does not hold is ignored rather than
    /// refused: the generated `finally` of a `lock` runs even on paths where
    /// `Enter` never completed, and throwing there would replace the real
    /// exception with a misleading one.
    pub fn exit(&self, object: Handle) {
        let me = std::thread::current().id();
        let mut state = self.lock();
        let key = object.to_bits();
        let Some((owner, depth)) = state.held.get_mut(&key) else { return };
        if *owner != me {
            return;
        }
        *depth -= 1;
        if *depth == 0 {
            state.held.remove(&key);
            drop(state);
            self.inner.released.notify_all();
        }
    }

    /// Whether this thread holds the monitor, for `Pulse` and diagnostics.
    pub fn holds(&self, object: Handle) -> bool {
        let me = std::thread::current().id();
        self.lock()
            .held
            .get(&object.to_bits())
            .is_some_and(|(owner, _)| *owner == me)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().expect("the monitor registry is poisoned")
    }
}

/// Without `std` there is one thread, so a monitor has nothing to exclude.
///
/// It still counts, because `lock` is re-entrant and the count is what tells
/// a nested `Exit` not to release too early.
#[cfg(not(feature = "std"))]
#[derive(Clone, Default)]
pub struct Monitors {
    inner: alloc::rc::Rc<core::cell::RefCell<HashMap<u64, usize>>>,
}

#[cfg(not(feature = "std"))]
impl Monitors {
    pub fn enter(&self, object: Handle, _blocked: &mut dyn FnMut(&mut dyn FnMut())) {
        *self.inner.borrow_mut().entry(object.to_bits()).or_insert(0) += 1;
    }

    pub fn exit(&self, object: Handle) {
        let mut held = self.inner.borrow_mut();
        if let Some(depth) = held.get_mut(&object.to_bits()) {
            *depth -= 1;
            if *depth == 0 {
                held.remove(&object.to_bits());
            }
        }
    }

    pub fn holds(&self, object: Handle) -> bool {
        self.inner.borrow().contains_key(&object.to_bits())
    }
}
