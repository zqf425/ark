//! Steering queue (design §7.1).
//!
//! A running turn can be corrected mid-flight: the reviewer (a human, the parent
//! agent, or an HTTP client) pushes text, and the reactor drains it at the next
//! safe boundary as an ordinary user message. Nothing is injected in the middle
//! of a model round, because that would desynchronize the tool-call/result pairs
//! the provider expects.
//!
//! The queue is a plain struct rather than a trait: both the reactor and the
//! delegation runtime need the *same* concrete handle, and a second trait would
//! only add indirection.

use std::collections::VecDeque;
use std::sync::Mutex;

/// A bounded queue of steering texts for one turn.
///
/// Bounded on purpose: an unbounded queue lets a caller grow a turn's context
/// without limit, which is exactly the failure mode `context_pressure` exists to
/// prevent.
#[derive(Debug, Default)]
pub struct SteerQueue {
    pending: Mutex<VecDeque<String>>,
    capacity: usize,
}

impl SteerQueue {
    /// Default capacity: enough for a few corrections, far below a context window.
    pub const DEFAULT_CAPACITY: usize = 16;

    pub fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            pending: Mutex::new(VecDeque::new()),
            capacity: capacity.max(1),
        }
    }

    /// Queue `text`. Returns `false` when the queue is full, so the caller can
    /// report back instead of silently dropping the instruction.
    pub fn push(&self, text: impl Into<String>) -> bool {
        let text = text.into();
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        let Ok(mut pending) = self.pending.lock() else {
            return false;
        };
        if pending.len() >= self.capacity {
            return false;
        }
        pending.push_back(text.to_string());
        true
    }

    /// Take everything queued so far, in arrival order.
    pub fn drain(&self) -> Vec<String> {
        let Ok(mut pending) = self.pending.lock() else {
            return Vec::new();
        };
        pending.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.pending
            .lock()
            .map(|pending| pending.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_in_arrival_order() {
        let queue = SteerQueue::new();
        assert!(queue.push("first"));
        assert!(queue.push("second"));
        assert_eq!(queue.drain(), vec!["first", "second"]);
        assert!(queue.is_empty());
    }

    #[test]
    fn blank_text_is_rejected() {
        let queue = SteerQueue::new();
        assert!(!queue.push("   "));
        assert!(queue.is_empty());
    }

    #[test]
    fn a_full_queue_reports_instead_of_dropping_silently() {
        let queue = SteerQueue::with_capacity(1);
        assert!(queue.push("a"));
        assert!(!queue.push("b"), "the caller must learn the queue is full");
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn concurrent_pushes_all_survive() {
        let queue = std::sync::Arc::new(SteerQueue::with_capacity(64));
        let mut handles = Vec::new();
        for index in 0..8 {
            let queue = std::sync::Arc::clone(&queue);
            handles.push(std::thread::spawn(move || {
                for round in 0..4 {
                    queue.push(format!("{index}-{round}"));
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(queue.len(), 32);
        assert_eq!(queue.drain().len(), 32);
    }
}
