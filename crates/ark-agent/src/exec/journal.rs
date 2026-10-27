//! The durable-event seam for one turn (design §7.4 / §7.9).
//!
//! `ark-agent` must not depend on `ark-store` (§5.2 dependency direction), so the
//! reactor talks to this narrow trait. `arkd` wires it to `SessionStore` /
//! `EventSink`; tests use [`MemoryWriter`].

use std::sync::{Arc, Mutex};

use ark_common::{Result, ThreadId};
use ark_contract::RuntimeEvent;

use crate::domain::action::TurnOutcome;

/// Append-only journal scoped to one turn.
#[async_trait::async_trait]
pub trait SessionWriter: Send + Sync {
    /// Append one event and return it with the assigned `seq`.
    async fn append(&self, event: RuntimeEvent) -> Result<RuntimeEvent>;

    /// Record the turn outcome. Implementations must write the matching terminal
    /// event (`turn_completed` / `turn_failed` / `turn_aborted`).
    ///
    /// Split from [`SessionWriter::append`] so the "terminal event exists"
    /// invariant has one enforcement point instead of being spread over three
    /// call sites in the reactor.
    async fn finish(&self, outcome: &TurnOutcome) -> Result<RuntimeEvent>;
}

/// Adapter from the port-level [`ark_ports::EventSink`] to one thread's journal.
pub struct EventSinkWriter {
    sink: Arc<dyn ark_ports::EventSink>,
    thread_id: ThreadId,
    events: crate::exec::events::TurnEvents,
}

impl EventSinkWriter {
    pub fn new(
        sink: Arc<dyn ark_ports::EventSink>,
        events: crate::exec::events::TurnEvents,
    ) -> Self {
        Self {
            sink,
            thread_id: events.thread_id,
            events,
        }
    }
}

#[async_trait::async_trait]
impl SessionWriter for EventSinkWriter {
    async fn append(&self, event: RuntimeEvent) -> Result<RuntimeEvent> {
        self.sink.publish(self.thread_id, event).await
    }

    async fn finish(&self, outcome: &TurnOutcome) -> Result<RuntimeEvent> {
        let event = match outcome {
            TurnOutcome::Completed => self.events.turn_completed(),
            TurnOutcome::Failed { message } => self.events.turn_failed(message.clone()),
            TurnOutcome::Aborted { reason } => self.events.turn_aborted(reason.clone()),
        };
        self.append(event).await
    }
}

/// In-memory journal for tests and for `--dry-run` wiring.
#[derive(Debug, Default)]
pub struct MemoryWriter {
    events: Mutex<Vec<RuntimeEvent>>,
    next_seq: Mutex<u64>,
    events_ctor: Option<crate::exec::events::TurnEvents>,
}

impl MemoryWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind the journal to a turn so `finish` can build terminal events.
    pub fn for_turn(events: crate::exec::events::TurnEvents) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            next_seq: Mutex::new(0),
            events_ctor: Some(events),
        }
    }

    /// Every event appended so far, in order.
    pub fn recorded(&self) -> Vec<RuntimeEvent> {
        self.events.lock().expect("journal mutex").clone()
    }

    /// Kinds of the recorded events, for readable assertions.
    pub fn kinds(&self) -> Vec<ark_contract::EventKind> {
        self.recorded().iter().map(RuntimeEvent::kind).collect()
    }

    pub fn last(&self) -> Option<RuntimeEvent> {
        self.recorded().last().cloned()
    }
}

#[async_trait::async_trait]
impl SessionWriter for MemoryWriter {
    async fn append(&self, event: RuntimeEvent) -> Result<RuntimeEvent> {
        let mut seq = self.next_seq.lock().expect("seq mutex");
        *seq += 1;
        let stored = event.with_seq(*seq);
        self.events
            .lock()
            .expect("journal mutex")
            .push(stored.clone());
        Ok(stored)
    }

    async fn finish(&self, outcome: &TurnOutcome) -> Result<RuntimeEvent> {
        let events = self
            .events_ctor
            .expect("MemoryWriter::for_turn is required to write terminals");
        let event = match outcome {
            TurnOutcome::Completed => events.turn_completed(),
            TurnOutcome::Failed { message } => events.turn_failed(message.clone()),
            TurnOutcome::Aborted { reason } => events.turn_aborted(reason.clone()),
        };
        self.append(event).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::events::TurnEvents;
    use ark_common::{ThreadId, TurnId};
    use ark_contract::EventKind;

    fn turn() -> TurnEvents {
        TurnEvents::new(ThreadId::new(), TurnId::new())
    }

    #[tokio::test]
    async fn memory_writer_assigns_increasing_seqs() {
        let events = turn();
        let writer = MemoryWriter::for_turn(events);
        let first = writer.append(events.turn_started()).await.unwrap();
        let second = writer
            .append(events.text_delta(ark_common::ItemId::new(), "hi"))
            .await
            .unwrap();
        assert_eq!(first.seq(), 1);
        assert_eq!(second.seq(), 2);
        assert_eq!(writer.recorded().len(), 2);
    }

    #[tokio::test]
    async fn finish_writes_the_matching_terminal_event() {
        let writer = MemoryWriter::for_turn(turn());
        let completed = writer.finish(&TurnOutcome::Completed).await.unwrap();
        assert_eq!(completed.kind(), EventKind::TurnCompleted);

        let failed = writer.finish(&TurnOutcome::failed("boom")).await.unwrap();
        assert_eq!(failed.kind(), EventKind::TurnFailed);

        let aborted = writer.finish(&TurnOutcome::aborted("stop")).await.unwrap();
        assert_eq!(aborted.kind(), EventKind::TurnAborted);
        assert_eq!(writer.kinds().len(), 3);
    }

    #[tokio::test]
    async fn last_returns_the_newest_event() {
        let events = turn();
        let writer = MemoryWriter::for_turn(events);
        writer.append(events.turn_started()).await.unwrap();
        assert_eq!(writer.last().unwrap().kind(), EventKind::TurnStarted);
    }
}
