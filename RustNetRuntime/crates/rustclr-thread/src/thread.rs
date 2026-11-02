//! `spawn`, `JoinHandle`, `Builder`, `current`, `sleep` -- std's shapes over
//! the embedder's thread hooks.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::any::Any;
use core::cell::UnsafeCell;
use core::fmt;
use core::num::NonZeroUsize;
use core::sync::atomic::{AtomicU32, Ordering};
use core::time::Duration;

use crate::hooks;
use crate::time::Instant;

/// std's `thread::Result`. A panic aborts without `std`, so `join` is always
/// `Ok`.
pub type Result<T> = core::result::Result<T, Box<dyn Any + Send + 'static>>;

/// Stack size when the caller does not ask for one.
pub const DEFAULT_STACK: usize = 8 << 20;

/// The id of a thread.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ThreadId(u64);

impl ThreadId {
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// A handle to a thread (the calling one, from [`current`]).
#[derive(Clone, Debug)]
pub struct Thread {
    id: ThreadId,
}

impl Thread {
    pub fn id(&self) -> ThreadId {
        self.id
    }
    pub fn name(&self) -> Option<&str> {
        None
    }
}

pub fn current() -> Thread {
    let id = if hooks::available() { (hooks::get().current_id)() } else { 0 };
    Thread { id: ThreadId(id) }
}

pub fn yield_now() {
    if hooks::available() {
        (hooks::get().yield_now)()
    } else {
        core::hint::spin_loop()
    }
}

/// Block the calling thread for at least `dur`.
pub fn sleep(dur: Duration) {
    let deadline = Instant::now() + dur;
    let word = AtomicU32::new(0);
    loop {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        // Nobody wakes this word: the wait ends by timing out.
        hooks::wait(&word, 0, Some((deadline - now).as_nanos() as u64));
        if !hooks::available() {
            return;
        }
    }
}

/// Processors the embedder reports.
pub fn available_parallelism() -> core::result::Result<NonZeroUsize, SpawnError> {
    let n = if hooks::available() { (hooks::get().parallelism)() } else { 1 };
    NonZeroUsize::new(n.max(1)).ok_or(SpawnError("no processors"))
}

/// A thread could not be started.
#[derive(Debug, Clone, Copy)]
pub struct SpawnError(&'static str);

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Where a finished thread leaves its result.
struct Packet<T> {
    /// 0 while running, 1 once `result` is written.
    done: AtomicU32,
    result: UnsafeCell<Option<T>>,
}

unsafe impl<T: Send> Sync for Packet<T> {}
unsafe impl<T: Send> Send for Packet<T> {}

/// An owned permission to join on a thread.
pub struct JoinHandle<T> {
    packet: Arc<Packet<T>>,
    token: u64,
    thread: Thread,
}

impl<T> JoinHandle<T> {
    /// Wait for the thread to finish and take its result.
    pub fn join(self) -> Result<T> {
        while self.packet.done.load(Ordering::Acquire) == 0 {
            hooks::wait(&self.packet.done, 0, None);
        }
        (hooks::get().join)(self.token);
        let value = unsafe { (*self.packet.result.get()).take() };
        Ok(value.expect("a finished thread left no result"))
    }

    pub fn is_finished(&self) -> bool {
        self.packet.done.load(Ordering::Acquire) != 0
    }

    pub fn thread(&self) -> &Thread {
        &self.thread
    }
}

impl<T> fmt::Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JoinHandle { .. }")
    }
}

/// What the new thread runs: the closure and where its result goes.
struct Start<F, T> {
    body: F,
    packet: Arc<Packet<T>>,
}

extern "C" fn trampoline<F, T>(arg: usize)
where
    F: FnOnce() -> T,
{
    let start = unsafe { Box::from_raw(arg as *mut Start<F, T>) };
    let Start { body, packet } = *start;
    let value = body();
    unsafe { *packet.result.get() = Some(value) };
    packet.done.store(1, Ordering::Release);
    hooks::wake(&packet.done, u32::MAX);
}

fn spawn_with<F, T>(stack: usize, body: F) -> core::result::Result<JoinHandle<T>, SpawnError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    if !hooks::available() {
        return Err(SpawnError("no thread hooks installed"));
    }
    let packet = Arc::new(Packet { done: AtomicU32::new(0), result: UnsafeCell::new(None) });
    let start = Box::into_raw(Box::new(Start { body, packet: packet.clone() }));
    let token = (hooks::get().spawn)(trampoline::<F, T>, start as usize, stack);
    if token == 0 {
        drop(unsafe { Box::from_raw(start) });
        return Err(SpawnError("the embedder could not start a thread"));
    }
    Ok(JoinHandle { packet, token, thread: Thread { id: ThreadId(token) } })
}

/// Start `body` on a new thread.
pub fn spawn<F, T>(body: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    spawn_with(DEFAULT_STACK, body).expect("failed to spawn thread")
}

/// std's thread builder: a name (kept for debugging only) and a stack size.
#[derive(Debug, Default)]
pub struct Builder {
    name: Option<String>,
    stack_size: Option<usize>,
}

impl Builder {
    pub fn new() -> Builder {
        Builder::default()
    }

    pub fn name(mut self, name: String) -> Builder {
        self.name = Some(name);
        self
    }

    pub fn stack_size(mut self, size: usize) -> Builder {
        self.stack_size = Some(size);
        self
    }

    pub fn spawn<F, T>(self, body: F) -> core::result::Result<JoinHandle<T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        spawn_with(self.stack_size.unwrap_or(DEFAULT_STACK).max(DEFAULT_STACK), body)
    }
}
