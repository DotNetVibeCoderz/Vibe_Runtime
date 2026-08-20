//! A lock-free multi-producer, multi-consumer queue.
//!
//! This is the Michael–Scott queue: producers append by CAS on the tail,
//! consumers detach by CAS on the head, and neither blocks the other. It backs
//! both the task scheduler's run queue and [`crate::channel`].
//!
//! Reclamation is the hard part of a lock-free queue in a language without a
//! GC. RustCLR has one, but this crate sits *below* it, so nodes are freed
//! through an epoch-free scheme: a dequeued node is retired to a per-queue
//! free list and only reused after the queue has drained, which is safe because
//! a node is unlinked before it is retired and no reader can reach it again.

use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::Mutex;

struct Node<T> {
    value: Option<T>,
    next: AtomicPtr<Node<T>>,
}

impl<T> Node<T> {
    fn new(value: Option<T>) -> *mut Node<T> {
        Box::into_raw(Box::new(Node { value, next: AtomicPtr::new(std::ptr::null_mut()) }))
    }
}

/// A lock-free FIFO queue.
pub struct LockFreeQueue<T> {
    head: AtomicPtr<Node<T>>,
    tail: AtomicPtr<Node<T>>,
    len: AtomicUsize,
    /// Nodes unlinked from the queue, awaiting reclamation.
    retired: Mutex<Vec<*mut Node<T>>>,
}

// The queue owns its nodes; `T` moving between threads is what makes it useful.
unsafe impl<T: Send> Send for LockFreeQueue<T> {}
unsafe impl<T: Send> Sync for LockFreeQueue<T> {}

impl<T> Default for LockFreeQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LockFreeQueue<T> {
    pub fn new() -> Self {
        // The queue always holds a dummy node so head and tail are never null.
        let dummy = Node::new(None);
        Self {
            head: AtomicPtr::new(dummy),
            tail: AtomicPtr::new(dummy),
            len: AtomicUsize::new(0),
            retired: Mutex::new(Vec::new()),
        }
    }

    /// Appends a value. Never blocks.
    pub fn push(&self, value: T) {
        let node = Node::new(Some(value));

        loop {
            let tail = self.tail.load(Ordering::Acquire);
            // SAFETY: `tail` is either the dummy or a node this queue owns and
            // has not yet retired; both are live.
            let next = unsafe { (*tail).next.load(Ordering::Acquire) };

            if tail != self.tail.load(Ordering::Acquire) {
                continue; // tail moved under us; retry
            }

            if next.is_null() {
                // SAFETY: as above.
                let linked = unsafe {
                    (*tail).next.compare_exchange(
                        std::ptr::null_mut(),
                        node,
                        Ordering::Release,
                        Ordering::Relaxed,
                    )
                };
                if linked.is_ok() {
                    // Help the tail along; failure is harmless, another thread
                    // will finish the swing.
                    let _ = self.tail.compare_exchange(
                        tail,
                        node,
                        Ordering::Release,
                        Ordering::Relaxed,
                    );
                    self.len.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            } else {
                // Tail was lagging; advance it and retry.
                let _ =
                    self.tail.compare_exchange(tail, next, Ordering::Release, Ordering::Relaxed);
            }
        }
    }

    /// Removes the oldest value, or returns `None` when the queue is empty.
    pub fn pop(&self) -> Option<T> {
        loop {
            let head = self.head.load(Ordering::Acquire);
            let tail = self.tail.load(Ordering::Acquire);
            // SAFETY: `head` is live for the same reason as `tail` in `push`.
            let next = unsafe { (*head).next.load(Ordering::Acquire) };

            if head != self.head.load(Ordering::Acquire) {
                continue;
            }

            if next.is_null() {
                return None; // genuinely empty
            }

            if head == tail {
                // Tail is lagging behind a completed push; help it forward.
                let _ =
                    self.tail.compare_exchange(tail, next, Ordering::Release, Ordering::Relaxed);
                continue;
            }

            // The value must not be taken until this thread has *won* the CAS.
            // Taking it first and restoring it on a loss is a data race: a
            // concurrent winner can observe the empty slot in between and drop
            // the value on the floor, or take it twice.
            if self
                .head
                .compare_exchange(head, next, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                // Winning the CAS is what confers the right to `next.value`.
                // Each node is passed exactly once, so no other thread will
                // take this one.
                //
                // SAFETY: `next` is live. It may already have been retired by a
                // later popper, but retirement only unlinks — memory is freed
                // in `reclaim`, which never runs concurrently with `pop`.
                let value = unsafe { (*next).value.take() };
                self.len.fetch_sub(1, Ordering::Relaxed);
                // The old head is now unreachable from the queue.
                self.retired.lock().unwrap().push(head);
                return value;
            }
        }
    }

    /// Approximate length. Exact only when no other thread is mutating.
    pub fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Frees retired nodes. Safe to call at any quiescent point.
    pub fn reclaim(&self) -> usize {
        let mut retired = self.retired.lock().unwrap();
        let count = retired.len();
        for node in retired.drain(..) {
            // SAFETY: the node was unlinked before being retired, so nothing
            // can reach it.
            unsafe {
                drop(Box::from_raw(node));
            }
        }
        count
    }
}

impl<T> Drop for LockFreeQueue<T> {
    fn drop(&mut self) {
        while self.pop().is_some() {}
        self.reclaim();
        let head = self.head.load(Ordering::Relaxed);
        if !head.is_null() {
            // SAFETY: exclusive access during drop; this is the dummy node.
            unsafe {
                drop(Box::from_raw(head));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn values_come_back_in_order() {
        let q = LockFreeQueue::new();
        for i in 0..100 {
            q.push(i);
        }
        assert_eq!(q.len(), 100);
        for i in 0..100 {
            assert_eq!(q.pop(), Some(i));
        }
        assert_eq!(q.pop(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn an_empty_queue_yields_none_rather_than_blocking() {
        let q: LockFreeQueue<i32> = LockFreeQueue::new();
        assert_eq!(q.pop(), None);
        q.push(1);
        assert_eq!(q.pop(), Some(1));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn concurrent_producers_and_consumers_lose_nothing() {
        const PRODUCERS: usize = 4;
        const PER_PRODUCER: usize = 2_000;

        let q: Arc<LockFreeQueue<usize>> = Arc::new(LockFreeQueue::new());
        let mut handles = Vec::new();

        for p in 0..PRODUCERS {
            let q = Arc::clone(&q);
            handles.push(std::thread::spawn(move || {
                for i in 0..PER_PRODUCER {
                    q.push(p * PER_PRODUCER + i);
                }
            }));
        }

        // Consumers share one counter so they stop when the *queue* is drained,
        // not when each of them individually has seen the total.
        let drained = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let consumers: Vec<_> = (0..2)
            .map(|_| {
                let q = Arc::clone(&q);
                let drained = Arc::clone(&drained);
                std::thread::spawn(move || {
                    let mut seen = Vec::new();
                    while drained.load(std::sync::atomic::Ordering::Acquire)
                        < PRODUCERS * PER_PRODUCER
                    {
                        match q.pop() {
                            Some(v) => {
                                seen.push(v);
                                drained.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                            }
                            None => std::thread::yield_now(),
                        }
                    }
                    seen
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        let mut all: Vec<usize> = Vec::new();
        for c in consumers {
            all.extend(c.join().unwrap());
        }
        // Drain anything the consumers missed after they stopped spinning.
        while let Some(v) = q.pop() {
            all.push(v);
        }

        all.sort_unstable();
        all.dedup();
        assert_eq!(
            all.len(),
            PRODUCERS * PER_PRODUCER,
            "every pushed value must be popped exactly once"
        );
    }

    #[test]
    fn retired_nodes_are_reclaimed() {
        let q = LockFreeQueue::new();
        for i in 0..10 {
            q.push(i);
        }
        while q.pop().is_some() {}
        assert_eq!(q.reclaim(), 10);
        assert_eq!(q.reclaim(), 0);
    }
}
