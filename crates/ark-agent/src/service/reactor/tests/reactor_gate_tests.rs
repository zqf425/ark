//! Gate-specific reactor tests kept in their own file so `reactor.rs` stays
//! within the file-size governance limit (AGENTS §4). Declared inside
//! `reactor::tests`, so `use super::*` reaches the shared test doubles and the
//! private `ask_approval`/`ask_user` methods (descendant module visibility).

use super::*;

use crate::exec::{ApprovalReply, MapToolRunner};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Gate double that tracks its live registrations, mirroring what an HTTP
/// broker exposes, so tests can assert a dropped future left nothing pending.
#[derive(Default)]
struct TrackingGates {
    pending: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

impl TrackingGates {
    fn pending_count(&self) -> usize {
        self.pending.lock().expect("pending mutex").len()
    }

    /// The shared registration set, so a writer can assert the ordering
    /// "gate id registered before its requested event is observable".
    fn shared_pending(&self) -> Arc<std::sync::Mutex<std::collections::HashSet<String>>> {
        Arc::clone(&self.pending)
    }
}

impl GateBroker for TrackingGates {
    fn prepare_approval(
        &self,
        request: ApprovalRequest,
    ) -> Result<crate::exec::GateFuture<ApprovalReply>> {
        assert!(self
            .pending
            .lock()
            .expect("pending mutex")
            .insert(request.approval_id.clone()));
        // Guard created before the future, mirroring production
        // `InteractiveGates::prepare`: an unpolled drop still cleans up.
        let guard = registration_guard(Arc::clone(&self.pending), request.approval_id.clone());
        Ok(Box::pin(async move {
            let _guard = guard;
            Ok(ApprovalReply::Approved)
        }))
    }

    fn prepare_user_input(
        &self,
        request: UserInputRequest,
    ) -> Result<crate::exec::GateFuture<String>> {
        assert!(self
            .pending
            .lock()
            .expect("pending mutex")
            .insert(request.request_id.clone()));
        let guard = registration_guard(Arc::clone(&self.pending), request.request_id.clone());
        Ok(Box::pin(async move {
            let _guard = guard;
            Ok("answered".to_string())
        }))
    }
}

/// Drop-time removal, mirroring ark-server's generation guard semantics.
fn registration_guard(
    pending: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    id: String,
) -> impl Drop {
    struct Guard(
        Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
        String,
    );
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.lock().expect("pending mutex").remove(&self.1);
        }
    }
    Guard(pending, id)
}

/// SessionWriter that locks in the gate ordering: when the first
/// `*_requested` event arrives, the matching id must already be registered in
/// the pending set shared with `TrackingGates`; otherwise the reactor appended
/// the observable event before preparing (the exact regression). The first
/// append then fails, proving the prepared future is dropped on the `?` after
/// prepare, clearing the registration.
struct FailFirstWriter {
    /// The `call_id` suffix the reactor embeds in the derived gate id, since
    /// the turn-id part is a random UUID that cannot be predicted here.
    expected_call_id: &'static str,
    pending: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    appends: AtomicUsize,
}

impl FailFirstWriter {
    fn new(
        expected_call_id: &'static str,
        pending: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    ) -> Self {
        Self {
            expected_call_id,
            pending,
            appends: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl SessionWriter for FailFirstWriter {
    async fn append(&self, event: RuntimeEvent) -> Result<RuntimeEvent> {
        match &event {
            RuntimeEvent::ApprovalRequested { approval_id, .. }
            | RuntimeEvent::UserInputRequested { request_id: approval_id, .. } => {
                assert!(
                    approval_id.ends_with(self.expected_call_id),
                    "unexpected gate id `{}` appended",
                    approval_id
                );
                assert!(
                    self.pending
                        .lock()
                        .expect("pending mutex")
                        .contains(approval_id),
                    "requested event became observable before the gate was registered"
                );
            }
            _ => unreachable!("these tests only append the requested event"),
        }
        if self.appends.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(ArkError::internal("injected append failure"));
        }
        unreachable!("ask_approval must abort before a second append")
    }

    async fn finish(&self, _outcome: &TurnOutcome) -> Result<RuntimeEvent> {
        unreachable!("these tests abort on the first append, never reaching finish")
    }
}

fn gate_test_reactor(gates: Arc<dyn GateBroker>, writer: Arc<dyn SessionWriter>) -> Reactor {
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(MapToolRunner::new()),
        writer,
        gates,
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
    })
}

#[tokio::test]
async fn approval_append_failure_drops_the_prepared_registration() {
    let gates = Arc::new(TrackingGates::default());
    // The writer fails on the requested event only after asserting the gate
    // id derived from `call-1` is already registered — locking the ordering.
    let writer = FailFirstWriter::new("call-1", gates.shared_pending());
    let mut reactor = gate_test_reactor(
        Arc::clone(&gates) as Arc<dyn GateBroker>,
        Arc::new(writer),
    );
    let call = ToolCallRequest::new("call-1", "bash", serde_json::json!({ "cmd": "dir" }))
        .with_sandbox(SandboxDecision::NeedsApproval);

    let error = reactor
        .ask_approval(call, "run dir".to_string())
        .await
        .unwrap_err();

    assert_eq!(error.code, ark_common::ArkErrorCode::InternalError);
    // The prepared future was dropped with the `?`: nothing left pending.
    assert_eq!(gates.pending_count(), 0);
}

#[tokio::test]
async fn user_input_append_failure_drops_the_prepared_registration() {
    let gates = Arc::new(TrackingGates::default());
    let writer = FailFirstWriter::new("call-1", gates.shared_pending());
    let mut reactor = gate_test_reactor(
        Arc::clone(&gates) as Arc<dyn GateBroker>,
        Arc::new(writer),
    );
    let call = ToolCallRequest::new("call-1", "request_user_input", serde_json::json!({}))
        .with_sandbox(SandboxDecision::ReadOnly);

    let error = reactor
        .ask_user(call, "which branch?".to_string())
        .await
        .unwrap_err();

    assert_eq!(error.code, ark_common::ArkErrorCode::InternalError);
    assert_eq!(gates.pending_count(), 0);
}
