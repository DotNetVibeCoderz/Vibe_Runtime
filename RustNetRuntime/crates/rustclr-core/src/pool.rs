//! The thread pool that runs `Task.Run` and `Parallel.*`.
//!
//! Before this, every `Task.Run` took an OS thread of its own. That is correct
//! and does not scale: a program starting a thousand tasks started a thousand
//! threads, and each one paid for a copy of the loader on the way up.
//!
//! # Why the workers hold interpreters
//!
//! A pool that ran plain closures would have to build an interpreter per job,
//! which is the cost being avoided — copying a loader is the expensive part of
//! starting a managed thread, not the thread. So each worker builds *one*
//! interpreter when the pool starts and reuses it for every job it runs. A job
//! is therefore `FnOnce(&mut Interpreter)`, not `FnOnce()`.
//!
//! # Idle workers and the collector
//!
//! A worker waiting for work holds no references into the heap, so it sits
//! inside [`Mutators::blocked`] and a collection does not wait for it. It
//! leaves that state before touching anything.
//!
//! # Waiting threads help
//!
//! A thread that blocks waiting for a task runs queued jobs while it waits.
//! Without that, a pool deadlocks in the classic way: every worker blocked on a
//! task whose job is still in the queue behind it. Helping means the work that
//! would unblock the waiter can always run, even if every worker is busy. It is
//! also why the pool does not need to grow under load.

use crate::host::{Host, SystemHost};
use crate::interp::Interpreter;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use rustclr_sched::LockFreeQueue;

/// One unit of work: something to do with a worker's interpreter.
type Job = Box<dyn FnOnce(&mut Interpreter) + Send>;

struct Inner {
    queue: LockFreeQueue<Job>,
    /// Work waiting for a deadline rather than a worker.
    timers: Mutex<Vec<(std::time::Instant, Job)>>,
    timer_wake: Condvar,
    /// Jobs queued or running. Zero means nothing is left to complete a task.
    outstanding: AtomicUsize,
    shutdown: AtomicBool,
    workers: AtomicUsize,
    lock: Mutex<()>,
    work_available: Condvar,
}

/// A pool of threads that run managed work.
#[derive(Clone)]
pub struct TaskPool {
    inner: Arc<Inner>,
}

impl TaskPool {
    /// Starts a pool sized to the machine, with workers built from `parent`.
    ///
    /// Each worker gets its own interpreter, which is a copy of the parent's
    /// loader plus a share of its heap and statics — the same arrangement
    /// [`Interpreter::worker`] makes for `Thread.Start`.
    pub fn start(parent: &Interpreter, host: impl Fn() -> Box<dyn Host> + Send + Sync + 'static) -> Self {
        let size = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let inner = Arc::new(Inner {
            queue: LockFreeQueue::new(),
            timers: Mutex::new(Vec::new()),
            timer_wake: Condvar::new(),
            outstanding: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            workers: AtomicUsize::new(0),
            lock: Mutex::new(()),
            work_available: Condvar::new(),
        });

        let host = Arc::new(host);
        for index in 0..size {
            // Built here, on the starting thread, so the worker is ready before
            // any job can be queued for it.
            let mut interp = parent.worker(host());
            let inner = Arc::clone(&inner);
            inner.workers.fetch_add(1, Ordering::AcqRel);
            let pool = TaskPool { inner: Arc::clone(&inner) };
            std::thread::Builder::new()
                .name(format!("rustclr-task-{index}"))
                .spawn(move || pool.run_worker(&mut interp))
                .expect("a task worker starts");
        }

        // One timer thread, however many delays are outstanding. It owns no
        // interpreter: when a deadline passes it queues the work like anything
        // else, so `Task.Delay`'s continuation runs on a pool worker.
        {
            let inner = Arc::clone(&inner);
            std::thread::Builder::new()
                .name("rustclr-timers".into())
                .spawn(move || run_timers(inner))
                .expect("the timer thread starts");
        }

        TaskPool { inner }
    }

    /// Runs `job` once `delay` has passed.
    ///
    /// Counted as outstanding straight away, so a thread waiting on the task
    /// this will complete knows something is still coming.
    pub fn schedule(&self, delay: std::time::Duration, job: Job) {
        self.inner.outstanding.fetch_add(1, Ordering::AcqRel);
        let due = std::time::Instant::now() + delay;
        {
            let mut timers = self.inner.timers.lock().expect("the timer list is poisoned");
            timers.push((due, job));
        }
        self.inner.timer_wake.notify_one();
    }

    pub fn worker_count(&self) -> usize {
        self.inner.workers.load(Ordering::Acquire)
    }

    /// Jobs queued or still running.
    pub fn outstanding(&self) -> usize {
        self.inner.outstanding.load(Ordering::Acquire)
    }

    /// Queues a job.
    pub fn submit(&self, job: Job) {
        self.inner.outstanding.fetch_add(1, Ordering::AcqRel);
        self.inner.queue.push(job);
        let _guard = self.inner.lock.lock().expect("the task pool is poisoned");
        self.inner.work_available.notify_one();
    }

    /// Runs one queued job on `interp`, if there is one.
    ///
    /// This is what lets a waiting thread help rather than idle, and what keeps
    /// the pool from deadlocking when every worker is blocked on a task whose
    /// job has not started.
    pub fn run_one(&self, interp: &mut Interpreter) -> bool {
        match self.inner.queue.pop() {
            Some(job) => {
                job(interp);
                self.inner.outstanding.fetch_sub(1, Ordering::AcqRel);
                true
            }
            None => false,
        }
    }

    fn run_worker(&self, interp: &mut Interpreter) {
        loop {
            if self.inner.shutdown.load(Ordering::Acquire) {
                return;
            }
            if self.run_one(interp) {
                continue;
            }

            // Nothing to do. Waiting holds no references into the heap, so the
            // collector is told not to wait for this thread.
            let idle = interp.blocked_now();
            {
                let guard = self.inner.lock.lock().expect("the task pool is poisoned");
                if self.inner.queue.is_empty() && !self.inner.shutdown.load(Ordering::Acquire) {
                    let _unused = self
                        .inner
                        .work_available
                        .wait_timeout(guard, std::time::Duration::from_millis(50))
                        .expect("the task pool is poisoned");
                }
            }
            drop(idle);
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _guard = self.lock.lock();
        self.work_available.notify_all();
    }
}

/// The host a pool worker writes through.
pub fn default_host() -> Box<dyn Host> {
    Box::new(SystemHost::new())
}

/// Moves work onto the queue as its deadline passes.
fn run_timers(inner: Arc<Inner>) {
    loop {
        if inner.shutdown.load(Ordering::Acquire) {
            return;
        }
        let mut due_now = Vec::new();
        let wait = {
            let mut timers = inner.timers.lock().expect("the timer list is poisoned");
            let now = std::time::Instant::now();
            let mut remaining = Vec::with_capacity(timers.len());
            for (at, job) in timers.drain(..) {
                if at <= now {
                    due_now.push(job);
                } else {
                    remaining.push((at, job));
                }
            }
            let next = remaining.iter().map(|(at, _)| *at).min();
            *timers = remaining;
            match next {
                Some(at) => at.saturating_duration_since(now),
                // Nothing pending: wake on the next `schedule`, or to re-check
                // shutdown. The cap is what makes that second case work.
                None => std::time::Duration::from_millis(50),
            }
        };

        for job in due_now {
            // Handing it to a worker rather than running it here: the timer
            // thread has no interpreter, and a continuation is managed code.
            inner.queue.push(job);
            let _guard = inner.lock.lock().expect("the task pool is poisoned");
            inner.work_available.notify_one();
        }

        let guard = inner.timers.lock().expect("the timer list is poisoned");
        let _unused = inner
            .timer_wake
            .wait_timeout(guard, wait.min(std::time::Duration::from_millis(50)))
            .expect("the timer list is poisoned");
    }
}
