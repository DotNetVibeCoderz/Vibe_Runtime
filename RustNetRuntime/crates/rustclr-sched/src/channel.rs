//! A multi-producer, multi-consumer channel over the lock-free queue.
//!
//! `std::sync::mpsc` is single-consumer, which does not match .NET's
//! `BlockingCollection` or `Channel<T>`; this one allows many of both.

use crate::queue::LockFreeQueue;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub struct SendError<T>(pub T);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvError {
    /// Every sender was dropped and the queue is drained.
    Disconnected,
    /// The timeout elapsed with nothing available.
    Timeout,
}

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "the channel is closed"),
            Self::Timeout => write!(f, "the receive timed out"),
        }
    }
}

impl std::error::Error for RecvError {}

struct Shared<T> {
    queue: LockFreeQueue<T>,
    senders: AtomicUsize,
    receivers: AtomicUsize,
    /// Only used to park a blocked receiver; the fast path never touches it.
    lock: Mutex<()>,
    ready: Condvar,
}

pub struct Sender<T> {
    shared: Arc<Shared<T>>,
}

pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
}

/// Creates a connected sender/receiver pair.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared {
        queue: LockFreeQueue::new(),
        senders: AtomicUsize::new(1),
        receivers: AtomicUsize::new(1),
        lock: Mutex::new(()),
        ready: Condvar::new(),
    });
    (
        Sender { shared: Arc::clone(&shared) },
        Receiver { shared },
    )
}

impl<T> Sender<T> {
    /// Sends a value. Fails only when every receiver has been dropped.
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        if self.shared.receivers.load(Ordering::Acquire) == 0 {
            return Err(SendError(value));
        }
        self.shared.queue.push(value);
        // Wake one parked receiver. The lock is taken only to avoid the lost
        // wakeup between a receiver's empty check and its wait.
        let _guard = self.shared.lock.lock().unwrap();
        self.shared.ready.notify_one();
        Ok(())
    }

    pub fn receiver_count(&self) -> usize {
        self.shared.receivers.load(Ordering::Relaxed)
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.shared.senders.fetch_add(1, Ordering::Relaxed);
        Sender { shared: Arc::clone(&self.shared) }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.shared.senders.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Last sender gone: release every parked receiver.
            let _guard = self.shared.lock.lock().unwrap();
            self.shared.ready.notify_all();
        }
    }
}

impl<T> Receiver<T> {
    /// Takes a value without blocking.
    pub fn try_recv(&self) -> Option<T> {
        self.shared.queue.pop()
    }

    /// Blocks until a value arrives or every sender is dropped.
    pub fn recv(&self) -> Result<T, RecvError> {
        loop {
            if let Some(v) = self.shared.queue.pop() {
                return Ok(v);
            }
            if self.shared.senders.load(Ordering::Acquire) == 0 {
                // Re-check: a value may have landed between the pop and this.
                return self.shared.queue.pop().ok_or(RecvError::Disconnected);
            }
            let guard = self.shared.lock.lock().unwrap();
            let _unused = self
                .shared
                .ready
                .wait_timeout(guard, Duration::from_millis(50))
                .unwrap();
        }
    }

    /// Blocks for at most `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvError> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(v) = self.shared.queue.pop() {
                return Ok(v);
            }
            if self.shared.senders.load(Ordering::Acquire) == 0 {
                return self.shared.queue.pop().ok_or(RecvError::Disconnected);
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err(RecvError::Timeout);
            }
            let guard = self.shared.lock.lock().unwrap();
            let _unused = self.shared.ready.wait_timeout(guard, deadline - now).unwrap();
        }
    }

    pub fn len(&self) -> usize {
        self.shared.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shared.queue.is_empty()
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        self.shared.receivers.fetch_add(1, Ordering::Relaxed);
        Receiver { shared: Arc::clone(&self.shared) }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.shared.receivers.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_arrive_in_order_from_one_sender() {
        let (tx, rx) = channel();
        for i in 0..10 {
            tx.send(i).unwrap();
        }
        for i in 0..10 {
            assert_eq!(rx.recv().unwrap(), i);
        }
    }

    #[test]
    fn dropping_every_sender_disconnects_the_receiver() {
        let (tx, rx) = channel::<i32>();
        tx.send(1).unwrap();
        drop(tx);
        assert_eq!(rx.recv().unwrap(), 1);
        assert_eq!(rx.recv(), Err(RecvError::Disconnected));
    }

    #[test]
    fn sending_to_a_dropped_receiver_returns_the_value() {
        let (tx, rx) = channel();
        drop(rx);
        assert_eq!(tx.send(7), Err(SendError(7)));
    }

    #[test]
    fn a_receive_blocks_until_a_sender_delivers() {
        let (tx, rx) = channel();
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            tx.send("late").unwrap();
        });
        assert_eq!(rx.recv().unwrap(), "late");
        sender.join().unwrap();
    }

    #[test]
    fn recv_timeout_gives_up() {
        let (_tx, rx) = channel::<i32>();
        assert_eq!(rx.recv_timeout(Duration::from_millis(10)), Err(RecvError::Timeout));
    }

    #[test]
    fn many_producers_reach_many_consumers() {
        let (tx, rx) = channel::<usize>();
        let mut senders = Vec::new();
        for p in 0..4 {
            let tx = tx.clone();
            senders.push(std::thread::spawn(move || {
                for i in 0..250 {
                    tx.send(p * 250 + i).unwrap();
                }
            }));
        }
        drop(tx);

        let mut consumers = Vec::new();
        for _ in 0..2 {
            let rx = rx.clone();
            consumers.push(std::thread::spawn(move || {
                let mut seen = Vec::new();
                while let Ok(v) = rx.recv() {
                    seen.push(v);
                }
                seen
            }));
        }
        drop(rx);

        for s in senders {
            s.join().unwrap();
        }
        let mut all = Vec::new();
        for c in consumers {
            all.extend(c.join().unwrap());
        }
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 1000);
    }
}
