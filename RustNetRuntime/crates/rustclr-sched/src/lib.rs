//! # rustclr-sched
//!
//! Threading, task scheduling and concurrency primitives for RustCLR.
//!
//! .NET's `Task`, `async`/`await` and `ThreadPool` all sit on one idea: a queue
//! of work items drained by a pool of worker threads. This crate provides that
//! substrate — a [`LockFreeQueue`] run queue, a [`ThreadPool`] that drains it,
//! and [`channel`] for handing values between threads.
//!
//! ```
//! use rustclr_sched::ThreadPool;
//! use std::sync::atomic::{AtomicUsize, Ordering};
//! use std::sync::Arc;
//!
//! let pool = ThreadPool::new(4);
//! let total = Arc::new(AtomicUsize::new(0));
//!
//! for i in 1..=100 {
//!     let total = Arc::clone(&total);
//!     pool.queue(move || { total.fetch_add(i, Ordering::Relaxed); });
//! }
//! pool.wait_for_idle();
//! assert_eq!(total.load(Ordering::Relaxed), 5050);
//! ```

pub mod channel;
pub mod queue;
pub mod task;

pub use channel::{channel, Receiver, RecvError, SendError, Sender};
pub use queue::LockFreeQueue;
pub use task::{JoinHandle, Task, TaskState, ThreadPool};
