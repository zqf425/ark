//! Turn control primitives shared by the agent loop and the delegation runtime
//! (design §7.1).
//!
//! These are concrete types, not traits, on purpose: both sides of the child-run
//! boundary need the *same* handle — the reactor awaits/drains it while the
//! runtime aborts/steers through it — and a trait would force the executor to
//! downcast or bridge-copy between two objects wrapping one signal.

use std::sync::Arc;

use tokio::sync::watch;

#[derive(Debug)]
struct Inner {
    sender: watch::Sender<Option<String>>,
}

/// A cloneable cancellation handle. Cloning shares the same signal.
#[derive(Debug, Clone)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelToken {
    pub fn new() -> Self {
        let (sender, _receiver) = watch::channel(None);
        Self {
            inner: Arc::new(Inner { sender }),
        }
    }

    /// Trip the signal with the ordinary user-interrupt reason.
    pub fn cancel(&self) {
        self.cancel_with_reason("turn interrupted");
    }

    /// Trip the signal with a durable reason. The first cancellation wins so a
    /// later shutdown cannot rewrite an earlier explicit user interrupt.
    pub fn cancel_with_reason(&self, reason: impl Into<String>) {
        let reason = reason.into();
        // `send_modify` holds the watch value's write lock while checking and
        // updating it, so concurrent interrupt/shutdown callers cannot replace
        // whichever reason arrived first. Unlike `send`, it also updates the
        // value when no receiver has subscribed yet.
        self.inner.sender.send_modify(|current| {
            if current.is_none() {
                *current = Some(reason);
            }
        });
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.sender.borrow().is_some()
    }

    /// The durable cancellation reason, when cancelled.
    pub fn reason(&self) -> Option<String> {
        self.inner.sender.borrow().clone()
    }

    /// Resolve when the token is cancelled. Cancel-safe: dropping this future
    /// never loses the signal, because the flag lives in the channel.
    pub async fn cancelled(&self) {
        let mut receiver = self.inner.sender.subscribe();
        if receiver.borrow().is_some() {
            return;
        }
        // Keep waiting while the channel is alive without a reason.
        while receiver.changed().await.is_ok() {
            if receiver.borrow().is_some() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    async fn token_starts_intact() {
        assert!(!CancelToken::new().is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_future_resolves_after_cancel() {
        let token = CancelToken::new();
        let waiter = token.clone();
        let handle = tokio::spawn(async move { waiter.cancelled().await });
        token.cancel();
        timeout(Duration::from_secs(1), handle)
            .await
            .expect("waiter resolved")
            .unwrap();
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_future_returns_immediately_when_already_cancelled() {
        let token = CancelToken::new();
        token.cancel();
        timeout(Duration::from_millis(50), token.cancelled())
            .await
            .expect("returns at once");
    }

    #[tokio::test]
    async fn cancel_is_idempotent() {
        let token = CancelToken::new();
        token.cancel();
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn clones_share_the_signal() {
        let token = CancelToken::new();
        let clone = token.clone();
        clone.cancel_with_reason("runtime shutting down");
        assert!(token.is_cancelled());
        assert_eq!(token.reason().as_deref(), Some("runtime shutting down"));
    }

    #[test]
    fn first_cancellation_reason_wins() {
        let token = CancelToken::new();
        token.cancel();
        token.cancel_with_reason("runtime shutting down");
        assert_eq!(token.reason().as_deref(), Some("turn interrupted"));
    }

    #[tokio::test]
    async fn pending_token_does_not_resolve_early() {
        let token = CancelToken::new();
        assert!(timeout(Duration::from_millis(20), token.cancelled())
            .await
            .is_err());
    }
}

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
mod steer_tests {
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
