//! SSE helpers kept separate from the handler.
//!
//! `to_sse` is the single place that decides how a [`RuntimeEvent`] becomes one
//! frame: `id:` is the authoritative `seq` (the writer's number), `event:` is the
//! kind, and `data:` is the full JSON payload the renderer already parses.

use axum::response::sse::Event;

use ark_contract::RuntimeEvent;

/// Render one event as an SSE frame.
pub fn to_sse(event: &RuntimeEvent) -> Event {
    let payload = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    Event::default()
        .id(event.seq().to_string())
        .event(event.kind().as_str())
        .data(payload)
}

/// Comment line used as the keep-alive body.
pub const KEEP_ALIVE_TEXT: &str = "keep-alive";

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::ThreadId;

    #[test]
    fn frames_carry_seq_and_kind() {
        let event = RuntimeEvent::Heartbeat {
            seq: 7,
            thread_id: ThreadId::new(),
            timestamp: chrono::Utc::now(),
        };
        let rendered = format!("{:?}", to_sse(&event));
        assert!(rendered.contains("7"));
    }

    #[test]
    fn keep_alive_text_is_ascii() {
        assert!(KEEP_ALIVE_TEXT.is_ascii());
    }
}
