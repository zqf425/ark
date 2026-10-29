//! Human-in-the-loop gates (design §7.1 `AwaitingApproval` / `AwaitingUserInput`).
//!
//! A gate is a blocking boundary call, never a polling loop: the reactor awaits
//! it inside a `tokio::select!` alongside the cancel token, so an interrupt
//! leaves a waiting gate immediately.

use std::future::Future;
use std::pin::Pin;

use ark_common::Result;

use crate::domain::tool::ToolCallRequest;

/// Ask a human to approve one tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub call: ToolCallRequest,
    pub summary: String,
}

impl ApprovalRequest {
    pub fn new(
        approval_id: impl Into<String>,
        call: ToolCallRequest,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            approval_id: approval_id.into(),
            call,
            summary: summary.into(),
        }
    }

    pub fn tool(&self) -> &str {
        &self.call.tool
    }
}

/// The human's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalReply {
    Approved,
    Denied { reason: String },
}

impl ApprovalReply {
    pub fn denied(reason: impl Into<String>) -> Self {
        ApprovalReply::Denied {
            reason: reason.into(),
        }
    }

    pub fn approved(&self) -> bool {
        matches!(self, ApprovalReply::Approved)
    }

    /// Reason text written into the synthetic tool result on denial.
    pub fn reason(&self) -> String {
        match self {
            ApprovalReply::Approved => String::new(),
            ApprovalReply::Denied { reason } => reason.clone(),
        }
    }
}

/// Ask a human a free-form question (the `request_user_input` tool).
#[derive(Debug, Clone, PartialEq)]
pub struct UserInputRequest {
    pub request_id: String,
    pub call: ToolCallRequest,
    pub prompt: String,
}

/// Future returned by [`GateBroker::prepare_*`]: the gate is already
/// registered when the future exists, and dropping the future (cancel, abort,
/// append failure) removes the registration — no 404 window, no leak.
pub type GateFuture<T> = Pin<Box<dyn Future<Output = Result<T>> + Send>>;

/// Boundary that resolves gates.
///
/// `prepare_*` is synchronous and must complete the registration **before**
/// returning: the reactor appends the `*_requested` event only after prepare,
/// so the moment a client sees the event the id is resolvable over HTTP.
/// The returned future resolves when the human answers; dropping it before
/// resolution removes the registration.
pub trait GateBroker: Send + Sync {
    /// Register an approval gate and return its wait future.
    fn prepare_approval(&self, request: ApprovalRequest) -> Result<GateFuture<ApprovalReply>>;

    /// Register a free-form input gate and return its wait future.
    fn prepare_user_input(&self, request: UserInputRequest) -> Result<GateFuture<String>>;
}

/// Gate implementation for unattended runs (`arkd --headless`, tests).
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAllGates;

impl GateBroker for AllowAllGates {
    fn prepare_approval(&self, _request: ApprovalRequest) -> Result<GateFuture<ApprovalReply>> {
        // No registration is needed: an unattended run has no human, so the
        // future is already resolved and dropping it cannot leak anything.
        Ok(Box::pin(async { Ok(ApprovalReply::Approved) }))
    }

    fn prepare_user_input(&self, request: UserInputRequest) -> Result<GateFuture<String>> {
        Ok(Box::pin(async move {
            Ok(format!(
                "unattended run: no user available for `{}`",
                request.request_id
            ))
        }))
    }
}

/// Gate implementation that refuses everything. Useful to prove the denial path
/// always yields a synthetic tool result instead of a dangling call.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAllGates;

impl GateBroker for DenyAllGates {
    fn prepare_approval(&self, _request: ApprovalRequest) -> Result<GateFuture<ApprovalReply>> {
        Ok(Box::pin(async {
            Ok(ApprovalReply::denied("approvals are disabled in this run"))
        }))
    }

    fn prepare_user_input(&self, request: UserInputRequest) -> Result<GateFuture<String>> {
        Ok(Box::pin(async move {
            Ok(format!(
                "approvals are disabled: `{}` was not answered",
                request.request_id
            ))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call() -> ToolCallRequest {
        ToolCallRequest::new("c1", "bash", serde_json::json!({ "command": "ls" }))
    }

    #[tokio::test]
    async fn allow_all_gates_approves() {
        let gates = AllowAllGates;
        let request = ApprovalRequest::new("a1", call(), "run ls");
        let reply = gates.prepare_approval(request).unwrap().await.unwrap();
        assert!(reply.approved());
        assert_eq!(reply.reason(), "");
    }

    #[tokio::test]
    async fn deny_all_gates_denies_with_a_reason() {
        let gates = DenyAllGates;
        let request = ApprovalRequest::new("a1", call(), "run ls");
        let reply = gates.prepare_approval(request).unwrap().await.unwrap();
        assert!(!reply.approved());
        assert!(reply.reason().contains("disabled"));
    }

    #[tokio::test]
    async fn allow_all_gates_answers_user_input() {
        let gates = AllowAllGates;
        let request = UserInputRequest {
            request_id: "r1".into(),
            call: call(),
            prompt: "which branch?".into(),
        };
        let answer = gates.prepare_user_input(request).unwrap().await.unwrap();
        assert!(answer.contains("r1"));
    }

    #[test]
    fn approval_request_exposes_the_tool() {
        assert_eq!(ApprovalRequest::new("a1", call(), "s").tool(), "bash");
    }
}
