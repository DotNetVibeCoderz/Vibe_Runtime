//! What the embedder supplies when there is no `std`.

use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

/// The operating-system services the threads, locks and clocks are built on.
/// Every function must be callable from any thread.
pub struct Hooks {
    /// Start `entry(arg)` on a new thread whose stack has at least
    /// `stack_size` bytes. When `entry` returns the thread must end. Returns a
    /// nonzero token for [`Hooks::join`], or 0 if no thread could be started.
    pub spawn: fn(entry: extern "C" fn(usize), arg: usize, stack_size: usize) -> u64,
    /// Release whatever the thread `token` still holds once it has ended (it
    /// is only called after `entry` returned).
    pub join: fn(token: u64),
    /// Block while `*word == expected`, for at most `timeout_ns` nanoseconds
    /// (`None`: no limit). May return early or spuriously; callers re-check.
    pub wait: fn(word: &AtomicU32, expected: u32, timeout_ns: Option<u64>),
    /// Wake up to `count` threads blocked in [`Hooks::wait`] on `word`.
    pub wake: fn(word: &AtomicU32, count: u32),
    /// A monotonic clock in nanoseconds.
    pub now_ns: fn() -> u64,
    /// An id for the calling thread, unique among live threads.
    pub current_id: fn() -> u64,
    /// Give the processor to another thread.
    pub yield_now: fn(),
    /// How many threads can run at once (processors).
    pub parallelism: fn() -> usize,
}

static HOOKS: AtomicPtr<Hooks> = AtomicPtr::new(core::ptr::null_mut());

/// Install the hooks. Call once, before the runtime starts a thread or takes
/// a lock that may be contended.
pub fn install(hooks: &'static Hooks) {
    HOOKS.store(hooks as *const Hooks as *mut Hooks, Ordering::Release);
}

/// Whether [`install`] has been called.
pub fn available() -> bool {
    !HOOKS.load(Ordering::Acquire).is_null()
}

pub(crate) fn get() -> &'static Hooks {
    let p = HOOKS.load(Ordering::Acquire);
    assert!(!p.is_null(), "rustclr-thread: no hooks installed (rustclr_thread::install)");
    unsafe { &*p }
}

/// Before the hooks exist (single-threaded start-up), waits are spins and
/// wake-ups go nowhere -- nobody else can be waiting yet.
pub(crate) fn wait(word: &AtomicU32, expected: u32, timeout_ns: Option<u64>) {
    if available() {
        (get().wait)(word, expected, timeout_ns)
    } else {
        core::hint::spin_loop();
    }
}

pub(crate) fn wake(word: &AtomicU32, count: u32) {
    if available() {
        (get().wake)(word, count)
    }
}

pub(crate) fn now_ns() -> u64 {
    if available() {
        (get().now_ns)()
    } else {
        0
    }
}
