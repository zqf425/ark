//! HTTP-resolvable approval and user-input gates.
//!
//! The reactor owns event ordering and cancellation; this bridge only pairs a
//! currently waiting request with its HTTP reply. Each waiter is removed by a
//! `Drop` guard, so interrupting a turn cannot leak a permanently pending gate.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use ark_agent::{ApprovalReply, ApprovalRequest, GateBroker, UserInputRequest};
use ark_common::{ArkError, Result};
use tokio::sync::oneshot;

type ApprovalWaiters = Arc<Mutex<HashMap<String, oneshot::Sender<ApprovalReply>>>>;
type InputWaiters = Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>;

/// In-process rendezvous used by the HTTP handlers and the parent reactor.
///
/// This is deliberately bounded by the number of active reactor gates: every
/// request has exactly one `oneshot` sender and cancellation drops it. It has
/// no polling loop or secondary state machine.
#[derive(Default)]
pub struct InteractiveGates {
    approvals: ApprovalWaiters,
    inputs: InputWaiters,
}

impl InteractiveGates {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve a known approval. `false` means it was already resolved,
    /// cancelled, or never existed.
    pub fn resolve_approval(&self, id: &str, approved: bool) -> bool {
        let sender = lock(&self.approvals).remove(id);
        sender
            .map(|sender| {
                sender
                    .send(if approved {
                        ApprovalReply::Approved
                    } else {
                        ApprovalReply::denied("rejected by the user")
                    })
                    .is_ok()
            })
            .unwrap_or(false)
    }

    /// Resolve a known free-form input request.
    pub fn resolve_user_input(&self, id: &str, answer: String) -> bool {
        let sender = lock(&self.inputs).remove(id);
        sender
            .map(|sender| sender.send(answer).is_ok())
            .unwrap_or(false)
    }

    #[cfg(test)]
    fn pending_approvals(&self) -> usize {
        lock(&self.approvals).len()
    }
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

struct ApprovalCleanup {
    id: String,
    waiters: ApprovalWaiters,
}

impl Drop for ApprovalCleanup {
    fn drop(&mut self) {
        lock(&self.waiters).remove(&self.id);
    }
}

struct InputCleanup {
    id: String,
    waiters: InputWaiters,
}

impl Drop for InputCleanup {
    fn drop(&mut self) {
        lock(&self.waiters).remove(&self.id);
    }
}

#[async_trait::async_trait]
impl GateBroker for InteractiveGates {
    async fn request_approval(&self, request: ApprovalRequest) -> Result<ApprovalReply> {
        let (sender, receiver) = oneshot::channel();
        {
            let mut approvals = lock(&self.approvals);
            if approvals.contains_key(&request.approval_id) {
                return Err(ArkError::conflict("approval id is already pending"));
            }
            approvals.insert(request.approval_id.clone(), sender);
        }
        let _cleanup = ApprovalCleanup {
            id: request.approval_id,
            waiters: Arc::clone(&self.approvals),
        };
        receiver
            .await
            .map_err(|_| ArkError::internal("approval was cancelled before resolution"))
    }

    async fn request_user_input(&self, request: UserInputRequest) -> Result<String> {
        let (sender, receiver) = oneshot::channel();
        {
            let mut inputs = lock(&self.inputs);
            if inputs.contains_key(&request.request_id) {
                return Err(ArkError::conflict("user-input id is already pending"));
            }
            inputs.insert(request.request_id.clone(), sender);
        }
        let _cleanup = InputCleanup {
            id: request.request_id,
            waiters: Arc::clone(&self.inputs),
        };
        receiver
            .await
            .map_err(|_| ArkError::internal("user input was cancelled before resolution"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn approval_waiter_receives_the_http_resolution() {
        let gates = std::sync::Arc::new(InteractiveGates::new());
        let waiting = std::sync::Arc::clone(&gates);
        let waiter = tokio::spawn(async move {
            waiting
                .request_approval(ark_agent::ApprovalRequest::new(
                    "approval-1",
                    ark_agent::ToolCallRequest::new("call-1", "bash", serde_json::json!({})),
                    "run command",
                ))
                .await
        });
        tokio::task::yield_now().await;

        assert!(gates.resolve_approval("approval-1", true));
        assert!(waiter.await.unwrap().unwrap().approved());
    }

    #[tokio::test]
    async fn cancelling_a_waiter_removes_its_pending_approval() {
        let gates = std::sync::Arc::new(InteractiveGates::new());
        let waiting = std::sync::Arc::clone(&gates);
        let waiter = tokio::spawn(async move {
            waiting
                .request_approval(ark_agent::ApprovalRequest::new(
                    "approval-cancel",
                    ark_agent::ToolCallRequest::new("call-1", "bash", serde_json::json!({})),
                    "run command",
                ))
                .await
        });
        tokio::task::yield_now().await;
        assert_eq!(gates.pending_approvals(), 1);

        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert_eq!(gates.pending_approvals(), 0);
        assert!(!gates.resolve_approval("approval-cancel", true));
    }
}
