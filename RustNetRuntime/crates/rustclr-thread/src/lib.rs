//! # rustclr-thread
//!
//! The threads, locks and clocks the multi-threaded runtime is written
//! against -- the part of `std::{sync, thread, time}` that RustCLR uses, and
//! nothing else.
//!
//! * **With `std`** (the default) every item is a plain re-export of std's,
//!   so a desktop build is exactly what it was before this crate existed.
//! * **Without `std`** the same items are implemented here over a handful of
//!   functions the *embedder* supplies once at start-up ([`Hooks`],
//!   [`install`]): start a thread, wait for one, block on a 32-bit word with
//!   an optional timeout (a futex), wake waiters, read a monotonic clock. An
//!   operating system that offers those -- Buitenzorg does, through its
//!   `THREAD_CREATE` / `FUTEX_WAIT_TIMEOUT` / `CLOCK_NANOS` syscalls -- gets
//!   real managed threads, a shared heap, stop-the-world collection,
//!   `Monitor.Wait` and the task pool, without a Rust standard library for
//!   its target.
//!
//! A microcontroller build does not use this crate at all: it compiles
//! RustCLR without the `threads` feature, where `Thread.Start` runs inline
//! and every lock is a `RefCell`.
//!
//! The implementation needs atomics (compare-and-swap) and `alloc`.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
pub mod sync {
    pub use std::sync::*;
}

#[cfg(feature = "std")]
pub mod thread {
    pub use std::thread::*;
}

#[cfg(feature = "std")]
pub mod time {
    pub use std::time::{Duration, Instant};
}

/// Whether the hooks are installed (always true with `std`).
#[cfg(feature = "std")]
pub fn available() -> bool {
    true
}

#[cfg(not(feature = "std"))]
mod hooks;
#[cfg(not(feature = "std"))]
pub use hooks::{available, install, Hooks};

#[cfg(not(feature = "std"))]
pub mod sync;
#[cfg(not(feature = "std"))]
pub mod thread;
#[cfg(not(feature = "std"))]
pub mod time;
