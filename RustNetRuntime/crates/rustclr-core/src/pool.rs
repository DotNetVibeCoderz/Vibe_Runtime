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
//!
//! # Where the work waits, and who is woken for it
//!
//! The queue is a [`RunQueue`] chosen at start — the scheduler is a swappable
//! module, as the collector is. Work stealing is the default; `--scheduler
//! fifo` puts back the single shared queue.
//!
//! Waking is deliberately stingy. Before, every submit took the pool's mutex
//! and signalled, and every signalled worker went looking for work at once.
//! With eight workers, a loop starting a hundred thousand one-line tasks ran
//! **twice as slowly** as with one, because the woken workers spent their
//! time contending for the heap and the queue rather than running anything.
//! Now a submit wakes a worker only when one is asleep *and* no other woken
//! worker is still looking for work; a worker that finds a job and sees more
//! waiting wakes the next. Work fans out as fast as there is work to take, and
//! no faster.
//!
//! # How many workers run at once
//!
//! Usually all of them. Every field a managed method reads or writes takes the
//! lock of the heap shard the object lives in. Threads mostly touch objects
//! they made themselves, which live in their own shard, so they mostly do not
//! meet — but a burst of tiny tasks that all touch shared objects can still
//! leave workers queueing more than working.
//!
//! Before the heap was sharded it was one lock, and this was the common case,
//! not the exception. Measured on this machine (8 cores), best of one run:
//!
//! | workload | 1 worker | 2 | 4 | 8 |
//! | --- | --- | --- | --- | --- |
//! | 100,000 `Task.Run` | 819 ms | 872 | 1,144 | 1,315 |
//! | fan-out of 1,000,000 tasks | 6,140 ms | 7,860 | 10,615 | 15,518 |
//!
//! So the pool measures. A governor on the timer thread reads, every tick,
//! what share of heap-lock acquisitions had to wait, and moves a limit on
//! running workers: halved above 20%, raised again below 10% while work is
//! waiting. A worker over the limit sleeps instead of taking a job. On the
//! sharded heap the fan-out waits on 4–11% and is never throttled.
//!
//! Two things keep the limit from ever costing correctness. A worker that
//! *blocks* inside a job gives its slot back (see [`TaskPool::worker_blocking`]),
//! so a limit of one cannot strand the job it is waiting for. And a tick that
//! finds work queued but nothing finished raises the limit whatever the
//! contention says, so a job spinning on a flag another queued job sets is
//! slowed, not hung.

use crate::host::Host;
#[cfg(feature = "std")]
use crate::host::SystemHost;
#[allow(unused_imports)]
use alloc::{boxed::Box, format, vec::Vec};
use crate::interp::Interpreter;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use rustclr_thread::sync::{Arc, Condvar, Mutex};

use rustclr_sched::{RunQueue, SchedulerKind};

/// One unit of work: something to do with a worker's interpreter.
type Job = Box<dyn FnOnce(&mut Interpreter) + Send>;

struct Inner {
    queue: Box<dyn RunQueue<Job>>,
    /// Workers asleep waiting for work.
    sleepers: AtomicUsize,
    /// Workers woken and not yet holding a job. While one is, a submit does
    /// not wake another: the one already looking will take this job, and wake
    /// the next itself if more is waiting.
    searching: AtomicUsize,
    /// Work waiting for a deadline rather than a worker.
    timers: Mutex<Vec<(rustclr_thread::time::Instant, Job)>>,
    timer_wake: Condvar,
    /// Jobs queued or running. Zero means nothing is left to complete a task.
    outstanding: AtomicUsize,
    shutdown: AtomicBool,
    workers: AtomicUsize,
    /// Workers not currently inside a blocking wait.
    ///
    /// A worker that blocks on I/O, a lock or a thread still occupies its slot
    /// but cannot take the next job. When every slot is occupied that way and
    /// work is queued, the queue stalls until one comes back — so the pool
    /// grows instead.
    running: AtomicUsize,
    /// Ceiling on growth. Unbounded growth turns a program that blocks a lot
    /// into a program that runs out of threads.
    ceiling: usize,
    lock: Mutex<()>,
    work_available: Condvar,
    /// Workers allowed to run a job at once. Moved by [`govern`].
    limit: AtomicUsize,
    /// Workers running a job and not blocked inside it.
    busy: AtomicUsize,
    /// Jobs finished, ever. The governor reads it as a rate.
    completed: AtomicUsize,
    /// The workers started at first, which is also the most the governor
    /// raises the limit to on contention grounds alone.
    size: usize,
    /// The heap, for its count of contended acquisitions.
    heap: rustclr_gc::SharedHeap,
    /// The first runtime failure inside a job that could not become a managed
    /// exception — an unresolvable token, a missing native. A job returns
    /// nothing, so without this such a failure vanished: the task it was
    /// running never completed, and a program could exit 0 halfway through.
    fatal: Mutex<Option<crate::error::ExecutionError>>,
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
    pub fn start(
        parent: &Interpreter,
        scheduler: SchedulerKind,
        host: impl Fn() -> Box<dyn Host> + Send + Sync + 'static,
    ) -> Self {
        let size = rustclr_thread::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let ceiling = (size * 8).max(16);
        let inner = Arc::new(Inner {
            // One queue slot per worker the pool could ever have, so a worker
            // started by growth has a queue of its own waiting for it.
            queue: scheduler.build(ceiling),
            sleepers: AtomicUsize::new(0),
            searching: AtomicUsize::new(0),
            timers: Mutex::new(Vec::new()),
            timer_wake: Condvar::new(),
            outstanding: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            workers: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            ceiling,
            lock: Mutex::new(()),
            work_available: Condvar::new(),
            limit: AtomicUsize::new(size),
            busy: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            size,
            heap: parent.heap.clone(),
            fatal: Mutex::new(None),
        });

        let host = Arc::new(host);
        for index in 0..size {
            // Built here, on the starting thread, so the worker is ready before
            // any job can be queued for it.
            let mut interp = parent.worker(host());
            let inner = Arc::clone(&inner);
            inner.workers.fetch_add(1, Ordering::AcqRel);
            let pool = TaskPool { inner: Arc::clone(&inner) };
            rustclr_thread::thread::Builder::new()
                .name(format!("rustclr-task-{index}"))
                .spawn(move || pool.run_worker(&mut interp, index))
                .expect("a task worker starts");
        }

        // One timer thread, however many delays are outstanding. It owns no
        // interpreter: when a deadline passes it queues the work like anything
        // else, so `Task.Delay`'s continuation runs on a pool worker.
        {
            let inner = Arc::clone(&inner);
            rustclr_thread::thread::Builder::new()
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
    pub fn schedule(&self, delay: rustclr_thread::time::Duration, job: Job) {
        self.inner.outstanding.fetch_add(1, Ordering::AcqRel);
        let due = rustclr_thread::time::Instant::now() + delay;
        {
            let mut timers = self.inner.timers.lock().expect("the timer list is poisoned");
            timers.push((due, job));
        }
        self.inner.timer_wake.notify_one();
    }

    pub fn worker_count(&self) -> usize {
        self.inner.workers.load(Ordering::Acquire)
    }

    /// Records a failure the program cannot continue past. The first wins:
    /// later ones are usually its consequences.
    pub fn record_fatal(&self, error: crate::error::ExecutionError) {
        let mut slot = self.inner.fatal.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_none() {
            *slot = Some(error);
        }
    }

    /// The failure [`Self::record_fatal`] kept, if any.
    pub fn fatal(&self) -> Option<crate::error::ExecutionError> {
        self.inner.fatal.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Jobs queued or still running.
    pub fn outstanding(&self) -> usize {
        self.inner.outstanding.load(Ordering::Acquire)
    }

    /// Which scheduling policy this pool runs.
    pub fn scheduler(&self) -> &'static str {
        self.inner.queue.name()
    }

    /// Queues a job. `worker` is the submitting thread's worker index, if it
    /// is one of this pool's workers; its job then goes on its own queue.
    pub fn submit(&self, job: Job, worker: Option<usize>) {
        self.inner.outstanding.fetch_add(1, Ordering::AcqRel);
        self.inner.queue.push(job, worker);
        self.wake_one_if_needed();
    }

    /// Wakes a sleeping worker, unless one already woken is still looking.
    ///
    /// Both counters are read after the push with `SeqCst`, and a worker
    /// going to sleep publishes itself in `sleepers` before its last look at
    /// the queue, also `SeqCst` — so either this sees the sleeper, or the
    /// sleeper sees the job. Neither can miss both.
    fn wake_one_if_needed(&self) {
        let inner = &self.inner;
        if inner.sleepers.load(Ordering::SeqCst) == 0 || inner.searching.load(Ordering::SeqCst) > 0 {
            return;
        }
        // A worker woken with every slot taken would only go back to sleep.
        if inner.busy.load(Ordering::SeqCst) >= inner.limit.load(Ordering::SeqCst) {
            return;
        }
        let _guard = inner.lock.lock().expect("the task pool is poisoned");
        inner.work_available.notify_one();
    }

    /// Called by a worker about to block, and again when it comes back.
    ///
    /// While it is away the pool is a worker short, and if that leaves nobody
    /// able to take queued work another worker is started. `parent` is the
    /// blocking worker's own interpreter, which is a perfectly good template —
    /// it has the loader, heap and statics a new one needs.
    pub fn worker_blocking(&self, parent: &Interpreter) -> Blocking<'_> {
        // Blocked, it is not running a job: its slot under the governor's
        // limit goes to whoever can use it, which may be the job it waits on.
        self.inner.busy.fetch_sub(1, Ordering::SeqCst);
        if !self.inner.queue.is_empty() {
            self.wake_one_if_needed();
        }
        let left = self.inner.running.fetch_sub(1, Ordering::AcqRel);
        // `left` is the value *before* the decrement, so one means this was the
        // last one running.
        if left <= 1 && !self.inner.queue.is_empty() {
            self.grow_from(parent);
        }
        Blocking { pool: self }
    }

    /// Starts one more worker, if the ceiling allows.
    fn grow_from(&self, parent: &Interpreter) {
        let workers = self.inner.workers.load(Ordering::Acquire);
        if workers >= self.inner.ceiling {
            return;
        }
        // A worker built from a blocked one: same heap, same statics, its own
        // frames and its own copy of the loader.
        let mut interp = parent.worker(parent.new_host());
        self.inner.workers.fetch_add(1, Ordering::AcqRel);
        self.inner.running.fetch_add(1, Ordering::AcqRel);
        let pool = TaskPool { inner: Arc::clone(&self.inner) };
        // `workers` was read before the increment, so it is this worker's
        // index, and below the ceiling the queue was sized for.
        let index = workers;
        let _ = rustclr_thread::thread::Builder::new()
            .name(format!("rustclr-task-{workers}"))
            .spawn(move || pool.run_worker_body(&mut interp, index));
    }

    /// Runs one queued job on `interp`, if there is one.
    ///
    /// This is what lets a waiting thread help rather than idle, and what keeps
    /// the pool from deadlocking when every worker is blocked on a task whose
    /// job has not started.
    pub fn run_one(&self, interp: &mut Interpreter) -> bool {
        match self.inner.queue.pop(interp.pool_worker_index()) {
            Some(job) => {
                // A job was queued by another thread and can name types that
                // thread made; see `Loader::catch_up`.
                interp.loader.catch_up();
                job(interp);
                self.inner.outstanding.fetch_sub(1, Ordering::AcqRel);
                true
            }
            None => false,
        }
    }

    fn run_worker(&self, interp: &mut Interpreter, index: usize) {
        self.inner.running.fetch_add(1, Ordering::AcqRel);
        self.run_worker_body(interp, index);
    }

    fn run_worker_body(&self, interp: &mut Interpreter, index: usize) {
        interp.mark_as_pool_worker(self.clone(), index);
        let inner = &self.inner;
        // Whether this worker was woken and has not yet found a job.
        let mut searching = false;
        loop {
            if inner.shutdown.load(Ordering::Acquire) {
                return;
            }
            let job = if self.claim_slot() {
                let job = inner.queue.pop(Some(index));
                if job.is_none() {
                    inner.busy.fetch_sub(1, Ordering::SeqCst);
                }
                job
            } else {
                None
            };
            if let Some(job) = job {
                if searching {
                    searching = false;
                    inner.searching.fetch_sub(1, Ordering::SeqCst);
                    // Found work, and stopped looking: if more is waiting,
                    // the next sleeper is this worker's to wake.
                    if !inner.queue.is_empty() {
                        self.wake_one_if_needed();
                    }
                }
                interp.loader.catch_up();
                job(interp);
                inner.busy.fetch_sub(1, Ordering::SeqCst);
                inner.completed.fetch_add(1, Ordering::Relaxed);
                inner.outstanding.fetch_sub(1, Ordering::AcqRel);
                continue;
            }
            if searching {
                searching = false;
                inner.searching.fetch_sub(1, Ordering::SeqCst);
            }

            // Nothing to do. Waiting holds no references into the heap, so the
            // collector is told not to wait for this thread.
            let idle = interp.blocked_now();

            // Spin a moment before sleeping. A producer handing out tiny tasks
            // one at a time — `for (…) Task.Run(…)` — otherwise pays a futex
            // wake per task, and the worker it wakes runs a microsecond of work
            // and sleeps again. A spinning worker counts as searching, so a
            // submit made meanwhile wakes nobody and this worker takes it.
            // It touches no heap, so it stays announced as blocked and never
            // holds up a collection.
            inner.searching.fetch_add(1, Ordering::SeqCst);
            let until = rustclr_thread::time::Instant::now() + IDLE_SPIN;
            let mut found = false;
            while rustclr_thread::time::Instant::now() < until {
                if inner.shutdown.load(Ordering::Acquire) {
                    break;
                }
                if !inner.queue.is_empty()
                    && inner.busy.load(Ordering::SeqCst) < inner.limit.load(Ordering::SeqCst)
                {
                    found = true;
                    break;
                }
                for _ in 0..32 {
                    core::hint::spin_loop();
                }
            }
            if found {
                // Still counted as searching: the job path stops counting and
                // wakes the next sleeper if more is queued.
                searching = true;
                drop(idle);
                continue;
            }
            inner.searching.fetch_sub(1, Ordering::SeqCst);
            {
                let guard = inner.lock.lock().expect("the task pool is poisoned");
                // Published before the last look, so a submit that pushed
                // after that look is certain to see a sleeper. See
                // `wake_one_if_needed`.
                inner.sleepers.fetch_add(1, Ordering::SeqCst);
                let barred = inner.busy.load(Ordering::SeqCst) >= inner.limit.load(Ordering::SeqCst);
                if (inner.queue.is_empty() || barred) && !inner.shutdown.load(Ordering::Acquire) {
                    let (_guard, timeout) = inner
                        .work_available
                        .wait_timeout(guard, rustclr_thread::time::Duration::from_millis(50))
                        .expect("the task pool is poisoned");
                    // Signalled rather than timed out: this worker was woken
                    // for a job, and counts as looking for it until it has one.
                    if !timeout.timed_out() {
                        searching = true;
                        inner.searching.fetch_add(1, Ordering::SeqCst);
                    }
                }
                inner.sleepers.fetch_sub(1, Ordering::SeqCst);
            }
            drop(idle);
        }
    }
}

impl TaskPool {
    /// Takes a slot under the governor's limit, if one is free.
    fn claim_slot(&self) -> bool {
        let inner = &self.inner;
        let mut busy = inner.busy.load(Ordering::SeqCst);
        loop {
            if busy >= inner.limit.load(Ordering::SeqCst) {
                return false;
            }
            match inner.busy.compare_exchange_weak(busy, busy + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return true,
                Err(now) => busy = now,
            }
        }
    }

    /// Workers currently allowed to run at once.
    pub fn concurrency_limit(&self) -> usize {
        self.inner.limit.load(Ordering::SeqCst)
    }
}

/// What the governor remembers between ticks.
#[derive(Default)]
struct Governor {
    contended: usize,
    acquisitions: usize,
    completed: usize,
}

/// How long an idle worker looks for work before it sleeps. Measured: with no
/// spin, 100,000 `Task.Run` took about 1,100 ms; with 15 µs, about 550 ms; with
/// 50 µs no faster, and a channel ping-pong — whose hops the spinning workers
/// compete with for the heap — about 15% slower.
const IDLE_SPIN: rustclr_thread::time::Duration = rustclr_thread::time::Duration::from_micros(15);

/// How often the governor looks.
const GOVERNOR_TICK: rustclr_thread::time::Duration = rustclr_thread::time::Duration::from_millis(20);

/// Percent of heap-lock acquisitions that had to wait, above which more
/// workers are mostly queueing, and below which more may help.
///
/// Measured on eight workers over the sharded heap: a fan-out of a million
/// tasks waited on 4–11% of acquisitions and finished in 2.4 s unthrottled
/// against 3.6 s throttled to one; 100,000 one-line `Task.Run`s waited on
/// 14–16%; a channel ping-pong about 4%. The bar sits above all of them, so
/// the governor acts only when contention is pathological.
///
/// It used to be "contended locks per *finished job*", which was the wrong
/// denominator: a job that runs long — a parent spawning four thousand
/// children — finishes nothing for a while, the ratio read as infinite, and
/// the pool throttled itself to one worker through the whole fan-out.
const WAITING_PERCENT_TOO_MANY: usize = 20;
const WAITING_PERCENT_ROOM: usize = 10;

/// One tick: moves the limit on running workers. See the module note.
fn govern(inner: &Inner, last: &mut Governor) {
    let contended = inner.heap.contended_acquisitions();
    let acquisitions = inner.heap.acquisitions();
    let completed = inner.completed.load(Ordering::Relaxed);
    let waited = contended.wrapping_sub(last.contended);
    let taken = acquisitions.wrapping_sub(last.acquisitions);
    let finished = completed.wrapping_sub(last.completed);
    last.contended = contended;
    last.acquisitions = acquisitions;
    last.completed = completed;

    let limit = inner.limit.load(Ordering::SeqCst);
    let backlog = inner.queue.len();
    let workers = inner.workers.load(Ordering::Acquire);
    let raise = |to: usize| {
        inner.limit.store(to, Ordering::SeqCst);
        let _guard = inner.lock.lock().expect("the task pool is poisoned");
        inner.work_available.notify_one();
    };
    // Too few acquisitions to say anything: leave the limit where it is.
    let measured = taken >= 1_000;

    if backlog > 0 && finished == 0 && limit < workers {
        // Work waiting and nothing finishing: whatever the reason, one more
        // worker. This is the rule that turns a would-be hang into a delay.
        raise(limit + 1);
    } else if measured && waited * 100 > taken * WAITING_PERCENT_TOO_MANY && limit > 1 {
        // Halved, not decremented — the rule TCP uses for the same problem.
        // A burst of tiny tasks is often over in a fraction of a second, and
        // stepping eight workers down one tick at a time spent most of it
        // with too many running.
        inner.limit.store((limit / 2).max(1), Ordering::SeqCst);
    } else if backlog > 0
        && (!measured || waited * 100 <= taken * WAITING_PERCENT_ROOM)
        && limit < inner.size.min(workers)
    {
        raise(limit + 1);
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
    #[cfg(feature = "std")]
    return Box::new(SystemHost::new());
    // No stdio without `std`: keep the output (an embedder that wants a
    // console installs `Interpreter::host_factory`).
    #[cfg(not(feature = "std"))]
    return Box::new(crate::host::CaptureHost::new());
}

/// Moves work onto the queue as its deadline passes.
fn run_timers(inner: Arc<Inner>) {
    let mut governor = Governor::default();
    let mut next_tick = rustclr_thread::time::Instant::now() + GOVERNOR_TICK;
    loop {
        if inner.shutdown.load(Ordering::Acquire) {
            return;
        }
        if rustclr_thread::time::Instant::now() >= next_tick {
            govern(&inner, &mut governor);
            next_tick = rustclr_thread::time::Instant::now() + GOVERNOR_TICK;
        }
        let mut due_now = Vec::new();
        let wait = {
            let mut timers = inner.timers.lock().expect("the timer list is poisoned");
            let now = rustclr_thread::time::Instant::now();
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
                None => rustclr_thread::time::Duration::from_millis(50),
            }
        };

        for job in due_now {
            // Handing it to a worker rather than running it here: the timer
            // thread has no interpreter, and a continuation is managed code.
            inner.queue.push(job, None);
            if inner.sleepers.load(Ordering::SeqCst) > 0 {
                let _guard = inner.lock.lock().expect("the task pool is poisoned");
                inner.work_available.notify_one();
            }
        }

        let guard = inner.timers.lock().expect("the timer list is poisoned");
        let _unused = inner
            .timer_wake
            // At most one governor tick, so the governor keeps its schedule
            // while no delay is outstanding.
            .wait_timeout(guard, wait.min(GOVERNOR_TICK))
            .expect("the timer list is poisoned");
    }
}

/// A worker's absence from the pool, restored when this drops.
pub struct Blocking<'a> {
    pool: &'a TaskPool,
}

impl Drop for Blocking<'_> {
    fn drop(&mut self) {
        self.pool.inner.running.fetch_add(1, Ordering::AcqRel);
        // Back in its job, and holding a slot again — over the limit for a
        // moment, possibly, which is the price of never stranding the job it
        // waited for. Without this `busy` only ever fell, wrapped below zero,
        // and no worker could claim a slot again.
        self.pool.inner.busy.fetch_add(1, Ordering::SeqCst);
    }
}
