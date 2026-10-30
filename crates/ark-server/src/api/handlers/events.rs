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
//!
//! A subscribe with an empty backlog still emits one control frame immediately,
//! so the response body yields bytes (and the client can report `open`) without
//! waiting for the keep-alive interval.

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
    // Synthetic control frames must never make a reconnecting client's
    // Last-Event-ID go backwards after replay or a live event.
    let mut last_sent_seq = watermark;
    if backlog.is_empty() {
        // An empty replay leaves the body byte-less until the first live event,
        // i.e. for a whole keep-alive interval on a quiet thread, and an
        // intermediary may hold the response back until then. One control frame
        // makes the body yield immediately. Only an empty replay sends it: a
        // replayed event already opens the stream, and repeating its `seq`
        // would hand the client a duplicate id.
        let opening = initial_frame(&thread_label, watermark);
        if sender.send(Ok(to_sse(&opening))).await.is_err() {
            return;
        }
    }
    for event in backlog {
        last_sent_seq = last_sent_seq.max(event.seq());
        if sender.send(Ok(to_sse(&event))).await.is_err() {
            return;
        }
    }
    tracing::debug!(thread = %thread_label, watermark, "sse switched to live");
    loop {
        // The receiver disappears the moment the client disconnects, but
        // `live.recv()` would keep waiting for the next event — forever when
        // `sse_idle_timeout_secs == 0` (idle timeout disabled). Watching
        // `sender.closed()` is what makes a quiet thread release its pump and
        // its store subscriber immediately instead of on the next fanout
        // (design §7.9, P1-10).
        let next = match idle {
            Some(limit) => tokio::select! {
                _ = sender.closed() => return,
                value = tokio::time::timeout(limit, live.recv()) => match value {
                    Ok(value) => value,
                    Err(_) => {
                        // Tell the client why the stream ends; it reconnects with
                        // its last `seq`, so nothing is lost.
                        let event = RuntimeEvent::SseIdleTimeout {
                            seq: last_sent_seq,
                            thread_id: parse_thread_lossy(&thread_label),
                            timestamp: chrono::Utc::now(),
                        };
                        let _ = sender.send(Ok(to_sse(&event))).await;
                        return;
                    }
                },
            },
            None => tokio::select! {
                _ = sender.closed() => return,
                value = live.recv() => value,
            },
        };
        match next {
            Some(event) => {
                last_sent_seq = last_sent_seq.max(event.seq());
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

/// First frame of a subscription whose backlog is empty.
///
/// It reuses `heartbeat` rather than introducing a subscribe-only wire type: the
/// contract already tells clients that heartbeats carry a `seq` but are not
/// display events. The `seq` is the subscribe watermark — the highest durable
/// number the store has committed — so the frame never claims a number the
/// writer has not issued and a client resuming from it cannot skip an event.
fn initial_frame(thread_label: &str, watermark: u64) -> RuntimeEvent {
    RuntimeEvent::Heartbeat {
        seq: watermark,
        thread_id: parse_thread_lossy(thread_label),
        timestamp: chrono::Utc::now(),
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
    use std::sync::Arc;

    use ark_common::ThreadId;
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use futures::StreamExt;
    use tower::ServiceExt;

    use super::*;
    use crate::state::{TurnEnd, TurnJob, TurnRunner};

    struct EmptyTools;

    impl ToolRegistryPort for EmptyTools {
        fn names(&self) -> Vec<String> {
            Vec::new()
        }

        fn schemas(&self) -> Vec<serde_json::Value> {
            Vec::new()
        }

        fn version(&self) -> u64 {
            0
        }
    }

    struct NoopRunner;

    #[async_trait::async_trait]
    impl TurnRunner for NoopRunner {
        async fn run_turn(&self, _job: TurnJob) -> ark_common::Result<TurnEnd> {
            Ok(TurnEnd::Completed)
        }
    }

    fn quiet_state(data_dir: &std::path::Path) -> AppState {
        let sessions =
            Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("session store"));
        let store: Arc<dyn SessionStore> = sessions;
        AppState::new(
            store,
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            Arc::new(NoopRunner),
            data_dir.to_path_buf(),
            "test",
        )
    }

    /// One SSE block split into `(field, value)` pairs of its non-comment lines.
    fn parse_block(raw: &str) -> Vec<(String, String)> {
        raw.lines()
            .filter(|line| !line.is_empty() && !line.starts_with(':'))
            .map(|line| match line.split_once(':') {
                Some((name, value)) => (name.to_string(), value.trim_start().to_string()),
                None => (line.to_string(), String::new()),
            })
            .collect()
    }

    /// Value of one SSE field, or `None` when the block omits it.
    fn sse_field<'a>(block: &'a [(String, String)], name: &str) -> Option<&'a str> {
        block
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

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
        assert!(!parse_thread_lossy("not-a-uuid").to_string().is_empty());
    }

    #[tokio::test]
    async fn idle_timeout_uses_the_highest_sent_sequence() {
        let thread_id = ThreadId::new();
        let (live_sender, live) = mpsc::channel(1);
        let (sender, mut receiver) = mpsc::channel(4);
        live_sender
            .send(RuntimeEvent::Heartbeat {
                seq: 11,
                thread_id,
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();
        let task = tokio::spawn(pump(
            ark_ports::SubscribeTicket {
                backlog: Vec::new(),
                watermark: 10,
                live,
            },
            sender,
            Some(Duration::from_millis(10)),
            thread_id.to_string(),
        ));

        // The empty backlog opens the stream with the watermark first.
        let opening = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("the opening frame must arrive before the idle timeout")
            .expect("pump must keep the stream open");
        assert!(format!("{opening:?}").contains("id: 10\\n"));

        let live_frame = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("live event must arrive before the idle timeout")
            .expect("pump must keep the stream open");
        assert!(format!("{live_frame:?}").contains("id: 11\\n"));
        let idle_frame = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("idle timeout frame must arrive")
            .expect("pump must emit a timeout frame");
        assert!(
            format!("{idle_frame:?}").contains("id: 11\\n"),
            "the synthetic timeout must not move the SSE id backwards"
        );
        task.await.unwrap();
    }

    /// The reported defect: a subscribed thread with nothing to replay produced
    /// no frame at all, so the client stayed in `connecting` until keep-alive.
    #[tokio::test]
    async fn empty_backlog_opens_the_stream_without_waiting_for_keep_alive() {
        let data_dir = std::env::temp_dir().join(format!("ark-sse-quiet-{}", ThreadId::new()));
        let state = quiet_state(&data_dir);
        let thread = state.store.create_thread("quiet").await.expect("thread");
        let thread_id = thread.thread_id;
        // Exactly the reported shape: a client that already read `last_seq` from
        // thread detail subscribes with `since_seq = last_seq`, so nothing is
        // replayed and the body used to stay empty until keep-alive.
        state
            .store
            .append_event(
                thread_id,
                RuntimeEvent::ThreadCreated {
                    seq: 0,
                    thread_id,
                    timestamp: chrono::Utc::now(),
                    title: "quiet".to_string(),
                },
            )
            .await
            .expect("thread_created persists");
        let last_seq = state.store.highest_seq(thread_id).await.expect("watermark");
        assert_eq!(
            last_seq, 1,
            "the created thread must have one durable event"
        );

        let response = crate::router(state)
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/threads/{thread_id}/events?since_seq={last_seq}"
                    ))
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("sse route responds");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );

        // The keep-alive interval defaults to 15s; the opening frame must not
        // depend on it, so two seconds is already generous.
        let mut stream = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("the first body frame must arrive well before keep-alive")
            .expect("the body must not end immediately")
            .expect("the first body frame must not be an error");

        let text = String::from_utf8_lossy(&chunk).to_string();
        let block = parse_block(text.trim_end());
        assert_eq!(
            sse_field(&block, "event"),
            Some("heartbeat"),
            "the opening frame must be an event the client already models"
        );
        assert_eq!(
            sse_field(&block, "id"),
            Some("1"),
            "the opening frame must reuse the subscribe watermark, never a new seq"
        );

        let payload: serde_json::Value = serde_json::from_str(
            sse_field(&block, "data").expect("the opening frame must carry data"),
        )
        .expect("the opening frame payload must be json");
        assert_eq!(payload["kind"], "heartbeat");
        assert_eq!(payload["seq"], 1);
        assert_eq!(payload["thread_id"], thread_id.to_string());

        drop(stream);
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    /// A replay already yields bytes, so no synthetic frame may be inserted —
    /// it would duplicate the backlog's last `seq` for every reconnect.
    #[tokio::test]
    async fn backlog_skips_the_opening_frame_and_keeps_order() {
        let thread_id = ThreadId::new();
        let backlog = [3u64, 4]
            .into_iter()
            .map(|seq| RuntimeEvent::Heartbeat {
                seq,
                thread_id,
                timestamp: chrono::Utc::now(),
            })
            .collect();
        let (_live_sender, live) = mpsc::channel(1);
        let (sender, mut receiver) = mpsc::channel(4);
        tokio::spawn(pump(
            ark_ports::SubscribeTicket {
                backlog,
                watermark: 4,
                live,
            },
            sender,
            None,
            thread_id.to_string(),
        ));

        let first = receiver.recv().await.expect("replayed frame");
        assert!(
            format!("{first:?}").contains("id: 3\\n"),
            "the first frame must be the oldest backlog entry, not a synthetic one"
        );
        let second = receiver.recv().await.expect("replayed frame");
        assert!(format!("{second:?}").contains("id: 4\\n"));
    }

    #[test]
    fn opening_frame_never_exceeds_the_watermark() {
        let frame = initial_frame(&ThreadId::new().to_string(), 7);
        assert_eq!(frame.seq(), 7);
        assert_eq!(frame.kind(), ark_contract::EventKind::Heartbeat);
    }
}
