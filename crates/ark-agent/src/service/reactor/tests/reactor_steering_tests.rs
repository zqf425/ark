//! Steering regressions: per-entry image ownership, image-preserving history,
//! and applying the normal context gate before a staged correction is sent.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn image_id(suffix: &str) -> AttachmentId {
    format!("019689f4-8000-7000-8000-{suffix}").parse().unwrap()
}

fn attachment_resolver() -> SteerAttachmentResolver {
    Arc::new(|ids: &[AttachmentId]| SteerAttachments {
        images: ids
            .iter()
            .map(|id| ModelImage {
                mime_type: "image/png".to_string(),
                data: id.to_string(),
            })
            .collect(),
        outcomes: Vec::new(),
    })
}

fn steering_reactor() -> (Reactor, Arc<MemoryWriter>, Arc<SteerQueue>) {
    let writer = Arc::new(MemoryWriter::new());
    let queue = Arc::new(SteerQueue::new());
    let mut reactor = reactor(Arc::clone(&writer));
    reactor.steer = Some(Arc::clone(&queue));
    reactor.steer_attachment_resolver = Some(attachment_resolver());
    (reactor, writer, queue)
}

#[tokio::test]
async fn each_staged_steer_receives_only_its_own_images() {
    let first = image_id("000000000001");
    let second = image_id("000000000002");
    let (mut reactor, _, queue) = steering_reactor();
    queue.push_with_attachments("first image", vec![first]);
    queue.push_with_attachments("second image", vec![second]);
    reactor.drain_steering();

    let request = reactor.request();
    let steered: Vec<_> = request
        .messages
        .iter()
        .filter(|message| message.role == ModelRole::User)
        .collect();

    assert_eq!(steered.len(), 2);
    assert_eq!(steered[0].images.len(), 1, "{:?}", steered[0].images);
    assert_eq!(steered[0].images[0].data, first.to_string());
    assert_eq!(steered[1].images.len(), 1, "{:?}", steered[1].images);
    assert_eq!(steered[1].images[0].data, second.to_string());
}

#[tokio::test]
async fn committing_steering_preserves_each_messages_images() {
    let first = image_id("000000000001");
    let second = image_id("000000000002");
    let (mut reactor, _, queue) = steering_reactor();
    queue.push_with_attachments("first image", vec![first]);
    queue.push_with_attachments("second image", vec![second]);
    reactor.drain_steering();

    reactor.commit_steering().await.unwrap();

    let steered: Vec<_> = reactor
        .history()
        .iter()
        .filter(|message| message.role == ModelRole::User)
        .collect();
    assert_eq!(steered.len(), 2);
    assert_eq!(steered[0].images.len(), 1, "{:?}", steered[0].images);
    assert_eq!(steered[0].images[0].data, first.to_string());
    assert_eq!(steered[1].images.len(), 1, "{:?}", steered[1].images);
    assert_eq!(steered[1].images[0].data, second.to_string());
}

struct CountingModel(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl ModelClient for CountingModel {
    fn provider_id(&self) -> &str {
        "test"
    }

    fn model(&self) -> &str {
        "test"
    }

    async fn stream(
        &self,
        _request: ModelRequest,
        _sink: mpsc::Sender<ModelChunk>,
    ) -> Result<ModelResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ModelResponse::default())
    }
}

#[tokio::test]
async fn staged_steering_cannot_bypass_the_context_gate() {
    let events = TurnEvents::new(ThreadId::new(), TurnId::new());
    let writer = Arc::new(MemoryWriter::for_turn(events));
    let calls = Arc::new(AtomicUsize::new(0));
    let queue = Arc::new(SteerQueue::new());
    let attachment = image_id("000000000003");
    let correction = "oversized correction ".repeat(1_000).trim().to_string();
    queue.push_with_attachments(correction.clone(), vec![attachment]);
    let mut reactor = reactor(writer.clone()).with_steering(Arc::clone(&queue));
    reactor.steer_attachment_resolver = Some(attachment_resolver());
    reactor.model = Arc::new(CountingModel(Arc::clone(&calls)));
    let estimator = TokenEstimator::new();
    let empty_estimate = estimator.estimate_request(&reactor.request());
    let mut steered_request = reactor.request();
    steered_request
        .messages
        .push(ModelMessage::user(correction.clone()));
    let steered_estimate = estimator.estimate_request(&steered_request);
    assert!(steered_estimate > empty_estimate);
    let limits = crate::domain::TurnLimits {
        context_limit_tokens: empty_estimate + 1,
        max_compactions: 0,
        ..crate::domain::TurnLimits::tiny()
    };
    reactor.machine = TurnMachine::new(limits);

    let outcome = reactor.run().await.unwrap();

    assert!(
        matches!(outcome, TurnOutcome::Failed { ref message, .. }
            if message.contains("context limit")),
        "the staged prompt must pass the context budget: {outcome:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "an oversized steer was sent"
    );
    assert!(!writer
        .recorded()
        .iter()
        .any(|event| matches!(event, RuntimeEvent::TurnSteered { .. })));
    let restored = queue.drain();
    assert_eq!(restored.len(), 1, "the rejected steer was dropped");
    assert_eq!(restored[0].text, correction);
    assert_eq!(restored[0].attachment_ids, vec![attachment]);
}

struct FailSteerWriter {
    inner: Arc<MemoryWriter>,
}

#[async_trait::async_trait]
impl SessionWriter for FailSteerWriter {
    async fn append(&self, event: RuntimeEvent) -> Result<RuntimeEvent> {
        if matches!(event, RuntimeEvent::TurnSteered { .. }) {
            return Err(ArkError::internal("injected steer commit failure"));
        }
        self.inner.append(event).await
    }

    async fn finish(&self, outcome: &TurnOutcome) -> Result<RuntimeEvent> {
        self.inner.finish(outcome).await
    }
}

#[tokio::test]
async fn a_steer_is_restored_when_its_commit_fails() {
    let events = TurnEvents::new(ThreadId::new(), TurnId::new());
    let writer = Arc::new(MemoryWriter::for_turn(events.clone()));
    let queue = Arc::new(SteerQueue::new());
    let attachment = image_id("000000000004");
    let correction = "keep this correction".to_string();
    queue.push_with_attachments(correction.clone(), vec![attachment]);
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(MapToolRunner::new()),
        writer: Arc::new(FailSteerWriter {
            inner: Arc::clone(&writer),
        }),
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        concurrency: None,
        steer_attachment_resolver: Some(attachment_resolver()),
    })
    .with_steering(Arc::clone(&queue));

    let outcome = reactor.run().await.unwrap();

    assert!(
        matches!(outcome, TurnOutcome::Failed { ref message, .. }
            if message == "injected steer commit failure"),
        "the injected write failure must terminate the turn: {outcome:?}"
    );
    let restored = queue.drain();
    assert_eq!(restored.len(), 1, "the uncommitted steer was dropped");
    assert_eq!(restored[0].text, correction);
    assert_eq!(restored[0].attachment_ids, vec![attachment]);
}
