//! `Mutex` and `Condvar` over the embedder's futex (wait on a 32-bit word,
//! wake its waiters), with std's signatures. Nothing here poisons: a panic
//! without `std` aborts, so a lock can never be left half-updated by an
//! unwinding thread, and the `LockResult`s are always `Ok`.

pub use alloc::sync::{Arc, Weak};
pub use core::sync::atomic;

use core::cell::UnsafeCell;
use core::fmt;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};
use core::time::Duration;

use crate::hooks;
use crate::time::Instant;

/// std's poison error. Never produced here; present for the signatures.
pub struct PoisonError<T> {
    guard: T,
}

impl<T> PoisonError<T> {
    pub fn new(guard: T) -> Self {
        PoisonError { guard }
    }
    pub fn into_inner(self) -> T {
        self.guard
    }
    pub fn get_ref(&self) -> &T {
        &self.guard
    }
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T> fmt::Debug for PoisonError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PoisonError")
    }
}

impl<T> fmt::Display for PoisonError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("poisoned lock")
    }
}

pub type LockResult<G> = Result<G, PoisonError<G>>;

pub enum TryLockError<T> {
    Poisoned(PoisonError<T>),
    WouldBlock,
}

impl<T> fmt::Debug for TryLockError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TryLockError::Poisoned(_) => f.write_str("Poisoned(..)"),
            TryLockError::WouldBlock => f.write_str("WouldBlock"),
        }
    }
}

pub type TryLockResult<G> = Result<G, TryLockError<G>>;

// ── Mutex ────────────────────────────────────────────────────────────────────

const UNLOCKED: u32 = 0;
const LOCKED: u32 = 1;
/// Locked, and at least one thread may be waiting in the futex.
const CONTENDED: u32 = 2;

/// A mutual-exclusion lock: the classic three-state futex mutex.
pub struct Mutex<T: ?Sized> {
    state: AtomicU32,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Mutex { state: AtomicU32::new(UNLOCKED), data: UnsafeCell::new(value) }
    }

    pub fn into_inner(self) -> LockResult<T> {
        Ok(self.data.into_inner())
    }
}

impl<T: ?Sized> Mutex<T> {
    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        if self.state.compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed).is_err() {
            self.lock_contended();
        }
        Ok(MutexGuard { mutex: self })
    }

    #[cold]
    fn lock_contended(&self) {
        // A short spin first: most critical sections here are a few
        // instructions long, and a syscall costs more than waiting them out.
        for _ in 0..100 {
            if self.state.compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                return;
            }
            core::hint::spin_loop();
        }
        while self.state.swap(CONTENDED, Ordering::Acquire) != UNLOCKED {
            hooks::wait(&self.state, CONTENDED, None);
        }
    }

    pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
        if self.state.compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            Ok(MutexGuard { mutex: self })
        } else {
            Err(TryLockError::WouldBlock)
        }
    }

    pub fn get_mut(&mut self) -> LockResult<&mut T> {
        Ok(self.data.get_mut())
    }

    pub fn is_poisoned(&self) -> bool {
        false
    }

    fn unlock(&self) {
        if self.state.swap(UNLOCKED, Ordering::Release) == CONTENDED {
            hooks::wake(&self.state, 1);
        }
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Mutex::new(T::default())
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Mutex { .. }")
    }
}

/// The lock held; dropping it unlocks.
pub struct MutexGuard<'a, T: ?Sized> {
    mutex: &'a Mutex<T>,
}

unsafe impl<T: ?Sized + Sync> Sync for MutexGuard<'_, T> {}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

// ── Condvar ──────────────────────────────────────────────────────────────────

/// Whether a timed wait ended by timing out.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct WaitTimeoutResult(bool);

impl WaitTimeoutResult {
    pub fn timed_out(&self) -> bool {
        self.0
    }
}

/// A condition variable: a sequence number that every notify bumps, so a
/// waiter that read it before releasing the mutex can never miss a notify
/// that came after. Spurious wake-ups are allowed (std allows them too).
pub struct Condvar {
    seq: AtomicU32,
}

impl Condvar {
    pub const fn new() -> Self {
        Condvar { seq: AtomicU32::new(0) }
    }

    pub fn wait<'a, T: ?Sized>(&self, guard: MutexGuard<'a, T>) -> LockResult<MutexGuard<'a, T>> {
        let mutex = guard.mutex;
        let seen = self.seq.load(Ordering::Acquire);
        drop(guard);
        hooks::wait(&self.seq, seen, None);
        mutex.lock()
    }

    pub fn wait_while<'a, T: ?Sized, F: FnMut(&mut T) -> bool>(
        &self,
        mut guard: MutexGuard<'a, T>,
        mut condition: F,
    ) -> LockResult<MutexGuard<'a, T>> {
        while condition(&mut *guard) {
            guard = match self.wait(guard) {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
        }
        Ok(guard)
    }

    pub fn wait_timeout<'a, T: ?Sized>(
        &self,
        guard: MutexGuard<'a, T>,
        dur: Duration,
    ) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
        let mutex = guard.mutex;
        let deadline = Instant::now() + dur;
        let seen = self.seq.load(Ordering::Acquire);
        drop(guard);
        let left = deadline.nanos().saturating_sub(Instant::now().nanos());
        if left > 0 {
            hooks::wait(&self.seq, seen, Some(left));
        }
        let timed_out = self.seq.load(Ordering::Acquire) == seen && Instant::now() >= deadline;
        let guard = match mutex.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        Ok((guard, WaitTimeoutResult(timed_out)))
    }

    pub fn wait_timeout_while<'a, T: ?Sized, F: FnMut(&mut T) -> bool>(
        &self,
        mut guard: MutexGuard<'a, T>,
        dur: Duration,
        mut condition: F,
    ) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
        let deadline = Instant::now() + dur;
        loop {
            if !condition(&mut *guard) {
                return Ok((guard, WaitTimeoutResult(false)));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok((guard, WaitTimeoutResult(true)));
            }
            guard = match self.wait_timeout(guard, deadline - now) {
                Ok((g, _)) => g,
                Err(e) => e.into_inner().0,
            };
        }
    }

    pub fn notify_one(&self) {
        self.seq.fetch_add(1, Ordering::Release);
        hooks::wake(&self.seq, 1);
    }

    pub fn notify_all(&self) {
        self.seq.fetch_add(1, Ordering::Release);
        hooks::wake(&self.seq, u32::MAX);
    }
}

impl Default for Condvar {
    fn default() -> Self {
        Condvar::new()
    }
}

impl fmt::Debug for Condvar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Condvar { .. }")
    }
}
