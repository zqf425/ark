//! `GET /v1/threads/{id}/events` — the SSE boundary (design §7.9).
//!
//! The replay/subscribe atomicity comes from the store: `subscribe` returns the
//! `(since_seq, watermark]` backlog and a live receiver that only yields
//! `seq > watermark` in one serialized step. This module therefore sends the
//! backlog first and then switches to live — already ordered, no de-duplication
//! on the client.
//!
//! A slow client is disconnected rather than back-pressuring the writer (the
//! store's live channel is bounded), and reconnects with its last `seq`.

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::mpsc;

use ark_contract::RuntimeEvent;

use crate::api::error::ApiError;
use crate::api::handlers::threads::parse_thread;
use crate::state::AppState;

/// Query parameters of the event stream.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventsQuery {
    /// Replay every durable event with `seq > since_seq` first.
    #[serde(default)]
    pub since_seq: Option<u64>,
}

/// `GET /v1/threads/{id}/events`.
pub async fn stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let thread_id = parse_thread(&id)?;
    state.store.get_thread(thread_id).await?;
    let since = query.since_seq.unwrap_or(0);
    let ticket = state.store.subscribe(thread_id, since).await?;
    let tuning = state.tuning();
    let heartbeat = Duration::from_secs(tuning.sse_heartbeat_secs.max(1));
    let idle = if tuning.sse_idle_timeout_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(tuning.sse_idle_timeout_secs))
    };

    let (sender, receiver) = mpsc::channel::<Result<Event, Infallible>>(64);
    let thread_label = thread_id.to_string();
    tokio::spawn(pump(ticket, sender, idle, thread_label));

    Ok(
        Sse::new(tokio_stream::wrappers::ReceiverStream::new(receiver))
            .keep_alive(KeepAlive::new().interval(heartbeat).text("keep-alive")),
    )
}

/// Send the backlog, then relay live events until the client goes away.
async fn pump(
    ticket: ark_ports::SubscribeTicket,
    sender: mpsc::Sender<Result<Event, Infallible>>,
    idle: Option<Duration>,
    thread_label: String,
) {
    let ark_ports::SubscribeTicket {
        backlog,
        watermark,
        mut live,
    } = ticket;
    for event in backlog {
        if sender.send(Ok(to_sse(&event))).await.is_err() {
            return;
        }
    }
    tracing::debug!(thread = %thread_label, watermark, "sse switched to live");
    loop {
        let next = match idle {
            Some(limit) => match tokio::time::timeout(limit, live.recv()).await {
                Ok(value) => value,
                Err(_) => {
                    // Tell the client why the stream ends; it reconnects with
                    // its last `seq`, so nothing is lost.
                    let event = RuntimeEvent::SseIdleTimeout {
                        seq: watermark,
                        thread_id: parse_thread_lossy(&thread_label),
                        timestamp: chrono::Utc::now(),
                    };
                    let _ = sender.send(Ok(to_sse(&event))).await;
                    return;
                }
            },
            None => live.recv().await,
        };
        match next {
            Some(event) => {
                if sender.send(Ok(to_sse(&event))).await.is_err() {
                    // Slow client: the store already disconnected it, so drop
                    // this connection and let it resume from `since_seq`.
                    return;
                }
            }
            None => return,
        }
    }
}

/// Render one event as an SSE frame. `id:` carries the authoritative `seq`.
fn to_sse(event: &RuntimeEvent) -> Event {
    let payload = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    Event::default()
        .id(event.seq().to_string())
        .event(event.kind().as_str())
        .data(payload)
}

/// Best-effort thread id for synthetic events; the label is always a UUID.
fn parse_thread_lossy(label: &str) -> ark_common::ThreadId {
    label
        .parse()
        .unwrap_or_else(|_| ark_common::ThreadId::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::ThreadId;

    #[test]
    fn sse_frames_carry_the_seq_as_id() {
        let event = RuntimeEvent::Heartbeat {
            seq: 42,
            thread_id: ThreadId::new(),
            timestamp: chrono::Utc::now(),
        };
        let frame = format!("{:?}", to_sse(&event));
        assert!(frame.contains("42"), "seq must appear as the frame id");
    }

    #[test]
    fn query_defaults_to_a_full_replay() {
        let query = EventsQuery::default();
        assert_eq!(query.since_seq.unwrap_or(0), 0);
    }

    #[test]
    fn lossy_parse_falls_back_to_a_fresh_id() {
        assert!(parse_thread_lossy("not-a-uuid").to_string().len() > 0);
    }
}
