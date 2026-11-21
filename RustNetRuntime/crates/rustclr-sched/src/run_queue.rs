//! Run queues: where queued work waits for a thread.
//!
//! The scheduler is a swappable module, like the collector. [`RunQueue`] is
//! the seam: a pool asks it for work and hands it work, and nothing else in
//! the runtime knows which policy is behind it. Two are provided.
//!
//! * [`GlobalFifo`] — one queue every thread shares. The original policy, and
//!   kept so that swapping back is a flag rather than a revert.
//! * [`WorkStealing`] — a queue per worker plus a shared injector. A worker
//!   pushes and pops its *own* queue at the back, newest first, and when that
//!   is empty takes from the injector and then steals from the *front* of the
//!   others'. This is the arrangement .NET's own thread pool uses, and for the
//!   same reasons: work a task spawns is usually wanted soon and touches what
//!   the task just touched, and a thread that only ever reaches into its own
//!   queue contends with nobody.
//!
//! Neither promises an order between jobs. `Task.Run` never has.
//!
//! The per-worker queues are a `Mutex<VecDeque>` each rather than a lock-free
//! Chase–Lev deque. The owner is nearly always the only one touching its
//! queue, so the lock is uncontended and costs an atomic swap; a lock-free
//! deque would save little of that and is the kind of code whose bugs are a
//! job run twice. Measured first, as `Plan.md` asks of 8.4, before anything
//! cleverer.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use rustclr_thread::sync::Mutex;

use crate::LockFreeQueue;

/// A place queued work waits.
///
/// `worker` identifies the calling thread when it is one of the pool's own
/// workers, and is `None` for any other thread — the main thread helping while
/// it waits, a thread started with `Thread.Start`, the timer.
pub trait RunQueue<J>: Send + Sync {
    /// Queues a job.
    fn push(&self, job: J, worker: Option<usize>);
    /// Takes a job, if any is queued anywhere this thread may take from.
    fn pop(&self, worker: Option<usize>) -> Option<J>;
    /// Jobs queued, everywhere. Exact once pushes and pops have finished;
    /// read with `SeqCst`, which the pool's wake-up protocol relies on.
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The policy's name, as `--scheduler` spells it.
    fn name(&self) -> &'static str;
}

/// Which policy to build. Parsed from `--scheduler`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SchedulerKind {
    /// One shared FIFO queue.
    Fifo,
    /// Per-worker queues with stealing.
    #[default]
    WorkStealing,
}

impl SchedulerKind {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "fifo" => Some(Self::Fifo),
            "stealing" | "work-stealing" => Some(Self::WorkStealing),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Fifo => "fifo",
            Self::WorkStealing => "stealing",
        }
    }

    /// Builds the queue for a pool that will never have more than `slots`
    /// workers.
    pub fn build<J: Send + 'static>(self, slots: usize) -> alloc::boxed::Box<dyn RunQueue<J>> {
        match self {
            Self::Fifo => alloc::boxed::Box::new(GlobalFifo::new()),
            Self::WorkStealing => alloc::boxed::Box::new(WorkStealing::new(slots)),
        }
    }
}

/// One queue for everyone: the pool as it was before work stealing.
pub struct GlobalFifo<J> {
    queue: LockFreeQueue<J>,
    len: AtomicUsize,
}

impl<J> GlobalFifo<J> {
    pub fn new() -> Self {
        Self { queue: LockFreeQueue::new(), len: AtomicUsize::new(0) }
    }
}

impl<J> Default for GlobalFifo<J> {
    fn default() -> Self {
        Self::new()
    }
}

impl<J: Send> RunQueue<J> for GlobalFifo<J> {
    fn push(&self, job: J, _worker: Option<usize>) {
        self.queue.push(job);
        self.len.fetch_add(1, Ordering::SeqCst);
    }

    fn pop(&self, _worker: Option<usize>) -> Option<J> {
        let job = self.queue.pop()?;
        self.len.fetch_sub(1, Ordering::SeqCst);
        Some(job)
    }

    fn len(&self) -> usize {
        self.len.load(Ordering::SeqCst)
    }

    fn name(&self) -> &'static str {
        "fifo"
    }
}

/// A queue per worker, an injector for everyone else, and stealing between
/// them.
pub struct WorkStealing<J> {
    /// Work from threads that are not pool workers.
    injector: LockFreeQueue<J>,
    /// One per worker slot, allocated up front so a pool that grows never has
    /// to move them.
    locals: Vec<Mutex<VecDeque<J>>>,
    len: AtomicUsize,
}

impl<J> WorkStealing<J> {
    pub fn new(slots: usize) -> Self {
        let mut locals = Vec::with_capacity(slots.max(1));
        for _ in 0..slots.max(1) {
            locals.push(Mutex::new(VecDeque::new()));
        }
        Self { injector: LockFreeQueue::new(), locals, len: AtomicUsize::new(0) }
    }

    fn local(&self, worker: Option<usize>) -> Option<&Mutex<VecDeque<J>>> {
        worker.and_then(|w| self.locals.get(w))
    }

    /// Takes the oldest job from some other worker's queue.
    fn steal(&self, thief: Option<usize>) -> Option<J> {
        let n = self.locals.len();
        let start = thief.map(|w| w + 1).unwrap_or(0);
        for k in 0..n {
            let victim = (start + k) % n;
            if Some(victim) == thief {
                continue;
            }
            // `try_lock`, not `lock`: a victim busy with its own queue is
            // skipped rather than waited for, and the next one asked.
            if let Ok(mut queue) = self.locals[victim].try_lock() {
                if let Some(job) = queue.pop_front() {
                    return Some(job);
                }
            }
        }
        None
    }
}

impl<J: Send> RunQueue<J> for WorkStealing<J> {
    fn push(&self, job: J, worker: Option<usize>) {
        match self.local(worker) {
            Some(local) => local.lock().expect("a run queue is poisoned").push_back(job),
            None => self.injector.push(job),
        }
        self.len.fetch_add(1, Ordering::SeqCst);
    }

    fn pop(&self, worker: Option<usize>) -> Option<J> {
        if self.len.load(Ordering::SeqCst) == 0 {
            return None;
        }
        let job = self
            .local(worker)
            .and_then(|local| local.lock().expect("a run queue is poisoned").pop_back())
            .or_else(|| self.injector.pop())
            .or_else(|| self.steal(worker))
            // A pass of `try_lock` can miss a job whose owner held its queue
            // for that instant. The count says one is there, so look once
            // more, waiting for each lock this time.
            .or_else(|| {
                self.locals.iter().enumerate().filter(|(i, _)| Some(*i) != worker).find_map(|(_, q)| {
                    q.lock().expect("a run queue is poisoned").pop_front()
                })
            })?;
        self.len.fetch_sub(1, Ordering::SeqCst);
        Some(job)
    }

    fn len(&self) -> usize {
        self.len.load(Ordering::SeqCst)
    }

    fn name(&self) -> &'static str {
        "stealing"
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    fn every_job_runs_exactly_once(kind: SchedulerKind) {
        let queue: Arc<dyn RunQueue<u64>> = Arc::from(kind.build::<u64>(4));
        let seen = Arc::new(AtomicU64::new(0));
        let count = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        for w in 0..4usize {
            let queue = Arc::clone(&queue);
            let seen = Arc::clone(&seen);
            let count = Arc::clone(&count);
            threads.push(std::thread::spawn(move || {
                for n in 0..10_000u64 {
                    queue.push(n, Some(w));
                    if n % 3 == 0 {
                        if let Some(v) = queue.pop(Some(w)) {
                            seen.fetch_add(v, Ordering::Relaxed);
                            count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        // Whatever is left, drained from outside the pool — which must be
        // able to reach every worker's queue, or a waiting thread could not
        // help.
        while let Some(v) = queue.pop(None) {
            seen.fetch_add(v, Ordering::Relaxed);
            count.fetch_add(1, Ordering::Relaxed);
        }
        assert_eq!(count.load(Ordering::Relaxed), 40_000);
        assert_eq!(seen.load(Ordering::Relaxed), 4 * (0..10_000u64).sum::<u64>());
        assert!(queue.is_empty());
    }

    #[test]
    fn fifo_runs_every_job_exactly_once() {
        every_job_runs_exactly_once(SchedulerKind::Fifo);
    }

    #[test]
    fn stealing_runs_every_job_exactly_once() {
        every_job_runs_exactly_once(SchedulerKind::WorkStealing);
    }

    #[test]
    fn a_worker_takes_its_own_newest_job_first_and_a_thief_the_oldest() {
        let queue = WorkStealing::<u32>::new(2);
        for n in 1..=3 {
            queue.push(n, Some(0));
        }
        assert_eq!(queue.pop(Some(0)), Some(3));
        assert_eq!(queue.pop(Some(1)), Some(1));
        assert_eq!(queue.pop(None), Some(2));
        assert_eq!(queue.pop(None), None);
    }
}
