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
    sender: watch::Sender<bool>,
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
        let (sender, _receiver) = watch::channel(false);
        Self {
            inner: Arc::new(Inner { sender }),
        }
    }

    /// Trip the signal. Idempotent: repeating it is harmless.
    pub fn cancel(&self) {
        // `send` (not `send_replace`) fails when nobody is subscribed yet, which
        // would silently drop the signal. `send_replace` always updates the
        // channel value, so a token cancelled before any waiter exists still
        // resolves every future waiter.
        self.inner.sender.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.inner.sender.borrow()
    }

    /// Resolve when the token is cancelled. Cancel-safe: dropping this future
    /// never loses the signal, because the flag lives in the channel.
    pub async fn cancelled(&self) {
        let mut receiver = self.inner.sender.subscribe();
        if *receiver.borrow() {
            return;
        }
        // Keep waiting while the channel is alive without a `true` value.
        while receiver.changed().await.is_ok() {
            if *receiver.borrow() {
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
        clone.cancel();
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn pending_token_does_not_resolve_early() {
        let token = CancelToken::new();
        assert!(timeout(Duration::from_millis(20), token.cancelled())
            .await
            .is_err());
    }
}
