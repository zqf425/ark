//! Cancellation primitive (design §7.1 "中断语义").
//!
//! `interrupt` only calls [`CancelToken::cancel`]; the state machine exits at
//! the next `tokio::select!` boundary. There is no scattered `aborted` flag that
//! half the code checks and half forgets.
//!
//! Implemented on `tokio::sync::watch` rather than `tokio-util` so the crate
//! keeps its dependency list to `ark-common`/`ark-contract`/`ark-ports` plus
//! tokio/serde/async-trait.

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
