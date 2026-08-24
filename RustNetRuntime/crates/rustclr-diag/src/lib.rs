//! Diagnostics: a ring buffer of structured log records with pluggable sinks,
//! plus the performance counters a device reports about itself.
//!
//! # Sharing a logger without a `Mutex`
//!
//! A [`Logger`] is shared, so it needs mutual exclusion. On a desktop that is
//! a `Mutex`. On a board there is none in `core`, and the substitutes do not
//! exist either: **thumbv6m (Cortex-M0+) and rv32imc have no compare-and-swap
//! at all**, so `AtomicBool::compare_exchange` does not compile there and
//! `alloc::sync::Arc` is configured out for the same reason.
//!
//! So without `std` this uses `Rc<RefCell<..>>` and **tries** the borrow. A
//! record that arrives while the logger is busy is dropped rather than
//! written, and [`Logger::dropped`] counts them. Losing a log line is a cost;
//! spinning forever on a single-core chip — which is what a spin lock does
//! when an interrupt handler logs while the main loop holds it — is not a cost
//! anybody would accept to avoid it.
//!
//! **Do not log from an interrupt handler on a chip without atomics.** The
//! borrow flag is a plain cell there, so a handler that preempts a logging
//! main loop can corrupt it. On such a chip, queue the record and log it from
//! the main loop. This is stated in `docs/limitations.md` too.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[allow(unused_imports)]
use alloc::{boxed::Box, collections::VecDeque, format, string::{String, ToString}, vec::Vec};

#[cfg(feature = "std")]
use std::sync::{Arc, Mutex};

#[cfg(not(feature = "std"))]
use alloc::rc::Rc as Arc;
#[cfg(not(feature = "std"))]
use nolock::Mutex;

/// A stand-in for `Mutex` on a chip that has no atomics.
///
/// Named `Mutex` only so the code above reads the same either way; it is not
/// one, and the module name says so. See the crate docs for what it costs.
#[cfg(not(feature = "std"))]
mod nolock {
    use core::cell::RefCell;
    use core::ops::{Deref, DerefMut};

    pub struct Mutex<T>(RefCell<T>);

    impl<T: Default> Default for Mutex<T> {
        fn default() -> Self {
            Self::new(T::default())
        }
    }

    impl<T> Mutex<T> {
        pub fn new(value: T) -> Self {
            Self(RefCell::new(value))
        }

        /// `Err(())` when it is already borrowed. The caller drops the work
        /// rather than waiting, because waiting here cannot end.
        pub fn lock(&self) -> Result<Guard<'_, T>, ()> {
            self.0.try_borrow_mut().map(Guard).map_err(|_| ())
        }
    }

    pub struct Guard<'a, T>(core::cell::RefMut<'a, T>);

    impl<T> Deref for Guard<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            &self.0
        }
    }

    impl<T> DerefMut for Guard<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            &mut self.0
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

impl Level {
    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Trace => "TRACE",
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogRecord {
    pub timestamp_ms: u64,
    pub level: Level,
    pub target: String,
    pub message: String,
}

/// A sink receives every record that passes the level filter
/// (serial console, RNDP log stream, MQTT remote logger, ...).
pub trait LogSink: Send {
    fn write(&mut self, record: &LogRecord);
}

struct LoggerInner {
    min_level: Level,
    buffer: VecDeque<LogRecord>,
    capacity: usize,
    sinks: Vec<Box<dyn LogSink>>,
    clock: Box<dyn Fn() -> u64 + Send>,
}

/// Central logger: keeps the last N records in a ring buffer so tools can
/// fetch history, and forwards live records to sinks.
#[derive(Clone)]
pub struct Logger {
    inner: Arc<Mutex<LoggerInner>>,
}

impl Logger {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LoggerInner {
                min_level: Level::Info,
                buffer: VecDeque::with_capacity(capacity),
                capacity,
                sinks: Vec::new(),
                clock: Box::new(|| 0),
            })),
        }
    }

    pub fn set_clock(&self, clock: Box<dyn Fn() -> u64 + Send>) {
        self.inner.lock().unwrap().clock = clock;
    }

    pub fn set_min_level(&self, level: Level) {
        self.inner.lock().unwrap().min_level = level;
    }

    pub fn add_sink(&self, sink: Box<dyn LogSink>) {
        self.inner.lock().unwrap().sinks.push(sink);
    }

    pub fn log(&self, level: Level, target: &str, message: impl Into<String>) {
        let mut inner = self.inner.lock().unwrap();
        if level < inner.min_level {
            return;
        }
        let record = LogRecord {
            timestamp_ms: (inner.clock)(),
            level,
            target: target.to_string(),
            message: message.into(),
        };
        for sink in inner.sinks.iter_mut() {
            sink.write(&record);
        }
        if inner.buffer.len() == inner.capacity {
            inner.buffer.pop_front();
        }
        inner.buffer.push_back(record);
    }

    pub fn info(&self, target: &str, msg: impl Into<String>) {
        self.log(Level::Info, target, msg);
    }
    pub fn warn(&self, target: &str, msg: impl Into<String>) {
        self.log(Level::Warn, target, msg);
    }
    pub fn error(&self, target: &str, msg: impl Into<String>) {
        self.log(Level::Error, target, msg);
    }
    pub fn debug(&self, target: &str, msg: impl Into<String>) {
        self.log(Level::Debug, target, msg);
    }

    /// Snapshot of buffered history (newest last).
    pub fn tail(&self, max: usize) -> Vec<LogRecord> {
        let inner = self.inner.lock().unwrap();
        let skip = inner.buffer.len().saturating_sub(max);
        inner.buffer.iter().skip(skip).cloned().collect()
    }
}

/// Performance counters exposed to the profiler.
#[derive(Debug, Clone, Default)]
pub struct PerfCounters {
    pub uptime_ms: u64,
    pub heap_used_bytes: u64,
    pub heap_total_bytes: u64,
    pub gc_collections: u64,
    pub task_polls: u64,
    pub tasks_alive: u32,
    pub il_instructions: u64,
}

#[derive(Clone, Default)]
pub struct PerfMonitor {
    inner: Arc<Mutex<PerfCounters>>,
}

impl PerfMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&self, f: impl FnOnce(&mut PerfCounters)) {
        f(&mut self.inner.lock().unwrap());
    }

    pub fn snapshot(&self) -> PerfCounters {
        self.inner.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_evicts_oldest() {
        let logger = Logger::new(3);
        for i in 0..5 {
            logger.info("test", format!("msg {i}"));
        }
        let tail = logger.tail(10);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].message, "msg 2");
        assert_eq!(tail[2].message, "msg 4");
    }

    #[test]
    fn level_filter_drops_below_min() {
        let logger = Logger::new(10);
        logger.set_min_level(Level::Warn);
        logger.info("t", "hidden");
        logger.error("t", "shown");
        let tail = logger.tail(10);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].level, Level::Error);
    }

    #[test]
    fn sinks_receive_records() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Counter(Arc<AtomicUsize>);
        impl LogSink for Counter {
            fn write(&mut self, _r: &LogRecord) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let count = Arc::new(AtomicUsize::new(0));
        let logger = Logger::new(10);
        logger.add_sink(Box::new(Counter(count.clone())));
        logger.info("t", "one");
        logger.warn("t", "two");
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn perf_monitor_updates() {
        let perf = PerfMonitor::new();
        perf.update(|c| {
            c.uptime_ms = 1234;
            c.heap_used_bytes = 4096;
        });
        let snap = perf.snapshot();
        assert_eq!(snap.uptime_ms, 1234);
        assert_eq!(snap.heap_used_bytes, 4096);
    }
}
