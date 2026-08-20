//! Tasks and the worker pool that runs them.
//!
//! This is the substrate .NET's `Task` and `async`/`await` sit on: work items
//! queued onto a lock-free run queue and drained by a fixed pool of workers.

use crate::queue::LockFreeQueue;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The lifecycle of a task, mirroring `System.Threading.Tasks.TaskStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Created,
    Queued,
    Running,
    Completed,
    Faulted,
}

type Work = Box<dyn FnOnce() + Send + 'static>;

/// Shared completion state for one queued task.
struct Completion<T> {
    value: Mutex<Option<std::thread::Result<T>>>,
    done: Condvar,
    state: Mutex<TaskState>,
}

/// A handle to a queued task's eventual result.
pub struct JoinHandle<T> {
    completion: Arc<Completion<T>>,
}

impl<T> JoinHandle<T> {
    /// Blocks until the task finishes, then returns its result.
    ///
    /// A task that panicked yields `Err`, mirroring how .NET surfaces a faulted
    /// task rather than tearing down the process.
    pub fn join(self) -> std::thread::Result<T> {
        let mut slot = self.completion.value.lock().unwrap();
        while slot.is_none() {
            slot = self.completion.done.wait(slot).unwrap();
        }
        slot.take().expect("completion is set exactly once")
    }

    /// Blocks for at most `timeout`. Returns `None` if the task is still going.
    pub fn join_timeout(&self, timeout: Duration) -> Option<std::thread::Result<T>> {
        let deadline = std::time::Instant::now() + timeout;
        let mut slot = self.completion.value.lock().unwrap();
        while slot.is_none() {
            let now = std::time::Instant::now();
            if now >= deadline {
                return None;
            }
            let (next, _) = self.completion.done.wait_timeout(slot, deadline - now).unwrap();
            slot = next;
        }
        slot.take()
    }

    pub fn state(&self) -> TaskState {
        *self.completion.state.lock().unwrap()
    }

    pub fn is_complete(&self) -> bool {
        matches!(self.state(), TaskState::Completed | TaskState::Faulted)
    }
}

/// A unit of queued work, kept for diagnostics.
#[derive(Debug, Clone, Copy)]
pub struct Task {
    pub id: u64,
    pub state: TaskState,
}

struct PoolShared {
    queue: LockFreeQueue<Work>,
    shutdown: AtomicBool,
    /// Tasks queued but not yet finished.
    outstanding: AtomicUsize,
    completed: AtomicUsize,
    next_id: AtomicUsize,
    lock: Mutex<()>,
    work_available: Condvar,
    idle: Condvar,
}

/// A fixed pool of worker threads draining a shared run queue.
pub struct ThreadPool {
    shared: Arc<PoolShared>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl ThreadPool {
    /// Creates a pool with `size` workers. A size of zero is raised to one, so
    /// queued work always makes progress.
    pub fn new(size: usize) -> Self {
        let size = size.max(1);
        let shared = Arc::new(PoolShared {
            queue: LockFreeQueue::new(),
            shutdown: AtomicBool::new(false),
            outstanding: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            next_id: AtomicUsize::new(1),
            lock: Mutex::new(()),
            work_available: Condvar::new(),
            idle: Condvar::new(),
        });

        let workers = (0..size)
            .map(|index| {
                let shared = Arc::clone(&shared);
                std::thread::Builder::new()
                    .name(format!("rustclr-worker-{index}"))
                    .spawn(move || worker_loop(shared))
                    .expect("worker thread spawns")
            })
            .collect();

        Self { shared, workers }
    }

    /// A pool sized to the machine.
    pub fn with_default_size() -> Self {
        Self::new(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
        )
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// Queues work with no result.
    pub fn queue<F>(&self, work: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.shared.outstanding.fetch_add(1, Ordering::AcqRel);
        let shared = Arc::clone(&self.shared);
        self.shared.queue.push(Box::new(move || {
            work();
            finish(&shared);
        }));
        let _guard = self.shared.lock.lock().unwrap();
        self.shared.work_available.notify_one();
    }

    /// Queues work and returns a handle to its result.
    pub fn spawn<F, T>(&self, work: F) -> JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let completion = Arc::new(Completion {
            value: Mutex::new(None),
            done: Condvar::new(),
            state: Mutex::new(TaskState::Queued),
        });

        let task_completion = Arc::clone(&completion);
        let shared = Arc::clone(&self.shared);
        self.shared.outstanding.fetch_add(1, Ordering::AcqRel);
        self.shared.next_id.fetch_add(1, Ordering::Relaxed);

        self.shared.queue.push(Box::new(move || {
            *task_completion.state.lock().unwrap() = TaskState::Running;
            // A panicking task must not poison the worker; catch it and report
            // it through the handle, the way a faulted `Task` does.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            *task_completion.state.lock().unwrap() = if result.is_ok() {
                TaskState::Completed
            } else {
                TaskState::Faulted
            };
            *task_completion.value.lock().unwrap() = Some(result);
            task_completion.done.notify_all();
            finish(&shared);
        }));

        let _guard = self.shared.lock.lock().unwrap();
        self.shared.work_available.notify_one();
        JoinHandle { completion }
    }

    /// Blocks until every queued item has finished.
    pub fn wait_for_idle(&self) {
        let mut guard = self.shared.lock.lock().unwrap();
        while self.shared.outstanding.load(Ordering::Acquire) > 0 {
            let (next, _) = self
                .shared
                .idle
                .wait_timeout(guard, Duration::from_millis(20))
                .unwrap();
            guard = next;
        }
    }

    pub fn queued_count(&self) -> usize {
        self.shared.queue.len()
    }

    pub fn completed_count(&self) -> usize {
        self.shared.completed.load(Ordering::Relaxed)
    }

    pub fn outstanding_count(&self) -> usize {
        self.shared.outstanding.load(Ordering::Relaxed)
    }
}

fn finish(shared: &Arc<PoolShared>) {
    shared.completed.fetch_add(1, Ordering::Relaxed);
    if shared.outstanding.fetch_sub(1, Ordering::AcqRel) == 1 {
        let _guard = shared.lock.lock().unwrap();
        shared.idle.notify_all();
    }
}

fn worker_loop(shared: Arc<PoolShared>) {
    loop {
        if let Some(work) = shared.queue.pop() {
            work();
            continue;
        }
        if shared.shutdown.load(Ordering::Acquire) {
            // Drain anything queued between the pop and the shutdown check.
            while let Some(work) = shared.queue.pop() {
                work();
            }
            return;
        }
        // Park briefly rather than spinning; the timeout also covers the race
        // between a producer's push and its notify.
        let guard = shared.lock.lock().unwrap();
        let _unused = shared
            .work_available
            .wait_timeout(guard, Duration::from_millis(10))
            .unwrap();
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        self.wait_for_idle();
        self.shared.shutdown.store(true, Ordering::Release);
        {
            let _guard = self.shared.lock.lock().unwrap();
            self.shared.work_available.notify_all();
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        self.shared.queue.reclaim();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn queued_work_all_runs() {
        let pool = ThreadPool::new(4);
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..1000 {
            let counter = Arc::clone(&counter);
            pool.queue(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            });
        }
        pool.wait_for_idle();
        assert_eq!(counter.load(Ordering::Relaxed), 1000);
        assert_eq!(pool.outstanding_count(), 0);
    }

    #[test]
    fn spawn_returns_the_computed_value() {
        let pool = ThreadPool::new(2);
        let handle = pool.spawn(|| (1..=10).sum::<i32>());
        assert_eq!(handle.join().unwrap(), 55);
    }

    #[test]
    fn a_panicking_task_faults_without_killing_the_pool() {
        let pool = ThreadPool::new(2);
        let bad = pool.spawn(|| -> i32 { panic!("intentional") });
        assert!(bad.join().is_err(), "the task should surface as faulted");

        // The pool must still be usable.
        let good = pool.spawn(|| 42);
        assert_eq!(good.join().unwrap(), 42);
    }

    #[test]
    fn task_state_advances_to_completed() {
        let pool = ThreadPool::new(1);
        let handle = pool.spawn(|| 1);
        assert_eq!(handle.join().unwrap(), 1);
    }

    #[test]
    fn join_timeout_reports_a_task_still_running() {
        let pool = ThreadPool::new(1);
        let handle = pool.spawn(|| {
            std::thread::sleep(Duration::from_millis(200));
            9
        });
        assert!(handle.join_timeout(Duration::from_millis(10)).is_none());
        assert_eq!(handle.join().unwrap(), 9);
    }

    #[test]
    fn a_zero_sized_pool_still_makes_progress() {
        let pool = ThreadPool::new(0);
        assert_eq!(pool.worker_count(), 1);
        assert_eq!(pool.spawn(|| 5).join().unwrap(), 5);
    }
}
