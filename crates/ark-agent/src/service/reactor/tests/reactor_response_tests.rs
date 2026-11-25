//! Final model text is authoritative for both provider history and the
//! completed assistant stream item, including providers that emit no deltas.

use super::*;

struct ConflictingTextModel {
    delta: Option<&'static str>,
    final_text: &'static str,
}

#[async_trait::async_trait]
impl ModelClient for ConflictingTextModel {
    fn provider_id(&self) -> &str {
        "test"
    }

    fn model(&self) -> &str {
        "test"
    }

    async fn stream(
        &self,
        _request: ModelRequest,
        sender: mpsc::Sender<ModelChunk>,
    ) -> Result<ModelResponse> {
        if let Some(delta) = self.delta {
            sender
                .send(ModelChunk::TextDelta {
                    delta: delta.to_string(),
                })
                .await
                .unwrap();
        }
        Ok(ModelResponse {
            text: self.final_text.to_string(),
            ..ModelResponse::default()
        })
    }
}

fn completed_assistant(writer: &MemoryWriter) -> String {
    writer
        .recorded()
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::ItemCompleted { item, .. } if item.kind == ItemKind::AssistantMessage => {
                item.text
            }
            _ => None,
        })
        .expect("a completed assistant item")
}

#[tokio::test]
async fn the_completed_stream_item_uses_the_authoritative_final_text() {
    let events = TurnEvents::new(ThreadId::new(), TurnId::new());
    let writer = Arc::new(MemoryWriter::for_turn(events));
    let mut reactor = reactor(writer.clone());
    reactor.model = Arc::new(ConflictingTextModel {
        delta: Some("streamed prefix"),
        final_text: "authoritative final answer",
    });
    reactor.push_user("hi");

    let outcome = reactor.run().await.unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    assert_eq!(completed_assistant(&writer), "authoritative final answer");
    assert_eq!(
        reactor
            .history()
            .last()
            .map(|message| message.content.as_str()),
        Some("authoritative final answer")
    );
}

#[tokio::test]
async fn a_nonempty_final_response_without_deltas_still_completes_an_item() {
    let events = TurnEvents::new(ThreadId::new(), TurnId::new());
    let writer = Arc::new(MemoryWriter::for_turn(events));
    let mut reactor = reactor(writer.clone());
    reactor.model = Arc::new(ConflictingTextModel {
        delta: None,
        final_text: "answer without deltas",
    });
    reactor.push_user("hi");

    let outcome = reactor.run().await.unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    assert_eq!(completed_assistant(&writer), "answer without deltas");
}
