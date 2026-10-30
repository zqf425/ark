//! Single-writer actor: the only place a `seq` is allocated.
//!
//! Design 7.4.3. One task per thread directory owns `WriterState`. Callers hold
//! a `WriterHandle` and can only send commands, so two concurrent appends can
//! never interleave, and `SnapshotAndSubscribe` cannot race an append: both are
//! serialized by the same `recv` loop.
//!
//! Appends are short synchronous file writes executed inside the actor. That is
//! deliberate: the actor is the single serialization point, so an fsync here
//! back-pressures writers (which is correct) and never touches subscribers,
//! whose channels are bounded and drained with `try_send`.

use crate::store::writer_state::{Snapshot, WriterState};
use ark_common::{ArkError, Result, ThreadId};
use ark_contract::{RewindResponse, RuntimeEvent};
use tokio::sync::{mpsc, oneshot};

/// Commands accepted by a writer actor (design 7.4.3, extended with the reads
/// the store facade needs so they also use the serialization point).
pub enum WriterCmd {
    Append {
        /// Boxed because `RuntimeEvent` is the largest payload by far: every
        /// slot of the writer command channel holds a `WriterCmd`, so an inline
        /// payload would cost 296 bytes per queued command even though most
        /// commands only carry a `u64`.
        event: Box<RuntimeEvent>,
        ack: oneshot::Sender<Result<RuntimeEvent>>,
    },
    SnapshotAndSubscribe {
        since_seq: u64,
        reply: oneshot::Sender<Result<Snapshot>>,
    },
    Watermark {
        reply: oneshot::Sender<u64>,
    },
    Replay {
        since_seq: u64,
        reply: oneshot::Sender<Vec<RuntimeEvent>>,
    },
    ChildCompletionEvents {
        reply: oneshot::Sender<Vec<RuntimeEvent>>,
    },
    InteractionEvents {
        reply: oneshot::Sender<Vec<RuntimeEvent>>,
    },
    Events {
        reply: oneshot::Sender<Vec<RuntimeEvent>>,
    },
    Compact {
        keep_last: usize,
        reply: oneshot::Sender<Result<Option<u64>>>,
    },
    Rewind {
        rewind_from_seq: u64,
        target_turn_id: Option<ark_common::TurnId>,
        reply: oneshot::Sender<Result<RewindResponse>>,
    },
    RewindRecord {
        rewind_from_seq: Option<u64>,
        target_turn_id: Option<ark_common::TurnId>,
        reply: oneshot::Sender<Option<RewindResponse>>,
    },
    /// Stop accepting work and drop the actor.
    Shutdown,
}

/// Cloneable handle to one thread writer.
#[derive(Clone)]
pub struct WriterHandle {
    thread_id: ThreadId,
    tx: mpsc::Sender<WriterCmd>,
}

impl WriterHandle {
    /// Spawn the actor task that owns `state`.
    pub fn spawn(state: WriterState, queue_capacity: usize) -> Self {
        let thread_id = state.thread_id();
        let (tx, rx) = mpsc::channel(queue_capacity.max(1));
        tokio::spawn(run(state, rx));
        Self { thread_id, tx }
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    /// Whether the actor task is still alive.
    pub fn is_alive(&self) -> bool {
        !self.tx.is_closed()
    }

    pub async fn append(&self, event: RuntimeEvent) -> Result<RuntimeEvent> {
        let (ack, rx) = oneshot::channel();
        self.send(WriterCmd::Append {
            event: Box::new(event),
            ack,
        })?;
        rx.await.map_err(|_| self.closed())?
    }

    /// Atomically watermark, read the backlog and register the live channel.
    pub async fn snapshot_and_subscribe(&self, since_seq: u64) -> Result<Snapshot> {
        let (reply, rx) = oneshot::channel();
        self.send(WriterCmd::SnapshotAndSubscribe { since_seq, reply })?;
        rx.await.map_err(|_| self.closed())?
    }

    pub async fn watermark(&self) -> Result<u64> {
        let (reply, rx) = oneshot::channel();
        self.send(WriterCmd::Watermark { reply })?;
        rx.await.map_err(|_| self.closed())
    }

    pub async fn replay(&self, since_seq: u64) -> Result<Vec<RuntimeEvent>> {
        let (reply, rx) = oneshot::channel();
        self.send(WriterCmd::Replay { since_seq, reply })?;
        rx.await.map_err(|_| self.closed())
    }

    /// Reads the child-completion lifecycle records owned by this writer.
    ///
    /// Returns: child completion and acknowledgement events in sequence order.
    pub async fn child_completion_events(&self) -> Result<Vec<RuntimeEvent>> {
        let (reply, rx) = oneshot::channel();
        self.send(WriterCmd::ChildCompletionEvents { reply })?;
        rx.await.map_err(|_| self.closed())
    }

    /// Reads only turn and interactive-gate lifecycle records from this writer.
    pub async fn interaction_events(&self) -> Result<Vec<RuntimeEvent>> {
        let (reply, rx) = oneshot::channel();
        self.send(WriterCmd::InteractionEvents { reply })?;
        rx.await.map_err(|_| self.closed())
    }

    pub async fn events(&self) -> Result<Vec<RuntimeEvent>> {
        let (reply, rx) = oneshot::channel();
        self.send(WriterCmd::Events { reply })?;
        rx.await.map_err(|_| self.closed())
    }

    pub async fn compact(&self, keep_last: usize) -> Result<Option<u64>> {
        let (reply, rx) = oneshot::channel();
        self.send(WriterCmd::Compact { keep_last, reply })?;
        rx.await.map_err(|_| self.closed())?
    }

    pub async fn rewind(
        &self,
        rewind_from_seq: u64,
        target_turn_id: Option<ark_common::TurnId>,
    ) -> Result<RewindResponse> {
        let (reply, rx) = oneshot::channel();
        self.send(WriterCmd::Rewind {
            rewind_from_seq,
            target_turn_id,
            reply,
        })?;
        rx.await.map_err(|_| self.closed())?
    }

    /// Look up a durable rewind marker without replaying compacted history.
    pub async fn rewind_record(
        &self,
        rewind_from_seq: Option<u64>,
        target_turn_id: Option<ark_common::TurnId>,
    ) -> Result<Option<RewindResponse>> {
        let (reply, rx) = oneshot::channel();
        self.send(WriterCmd::RewindRecord {
            rewind_from_seq,
            target_turn_id,
            reply,
        })?;
        rx.await.map_err(|_| self.closed())
    }

    /// Ask the actor to stop, then wait for the queue to drain.
    pub async fn shutdown(&self) {
        let _ = self.tx.send(WriterCmd::Shutdown).await;
        self.tx.closed().await;
    }

    fn send(&self, cmd: WriterCmd) -> Result<()> {
        self.tx.try_send(cmd).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => ArkError::store(format!(
                "writer queue for thread {} is full",
                self.thread_id
            )),
            mpsc::error::TrySendError::Closed(_) => {
                ArkError::store(format!("writer for thread {} has stopped", self.thread_id))
            }
        })
    }

    fn closed(&self) -> ArkError {
        ArkError::store(format!(
            "writer for thread {} dropped the reply",
            self.thread_id
        ))
    }
}

async fn run(mut state: WriterState, mut rx: mpsc::Receiver<WriterCmd>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            WriterCmd::Append { event, ack } => {
                let result = state.append(*event);
                let _ = ack.send(result);
            }
            WriterCmd::SnapshotAndSubscribe { since_seq, reply } => {
                let result = state.snapshot_and_subscribe(since_seq);
                let _ = reply.send(result);
            }
            WriterCmd::Watermark { reply } => {
                let _ = reply.send(state.watermark());
            }
            WriterCmd::Replay { since_seq, reply } => {
                let _ = reply.send(state.replay(since_seq));
            }
            WriterCmd::ChildCompletionEvents { reply } => {
                let _ = reply.send(state.child_completion_events());
            }
            WriterCmd::InteractionEvents { reply } => {
                let _ = reply.send(state.interaction_events());
            }
            WriterCmd::Events { reply } => {
                let _ = reply.send(state.events().to_vec());
            }
            WriterCmd::Compact { keep_last, reply } => {
                let result = state.compact(keep_last);
                let _ = reply.send(result);
            }
            WriterCmd::Rewind {
                rewind_from_seq,
                target_turn_id,
                reply,
            } => {
                let result = state.rewind(rewind_from_seq, target_turn_id);
                let _ = reply.send(result);
            }
            WriterCmd::RewindRecord {
                rewind_from_seq,
                target_turn_id,
                reply,
            } => {
                let _ = reply.send(state.rewind_record(rewind_from_seq, target_turn_id));
            }
            WriterCmd::Shutdown => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StoreLimits;
    use crate::domain::diagnostics::MemoryDiagnostics;
    use crate::domain::layout::ThreadPaths;
    use crate::store::tests::{event_at, TempDir};
    use std::sync::Arc;

    fn handle(dir: &TempDir, thread: ThreadId) -> WriterHandle {
        let state = WriterState::open(
            thread,
            ThreadPaths::new(dir.path().join("thread")),
            StoreLimits::for_tests(),
            Arc::new(MemoryDiagnostics::new()),
        )
        .unwrap();
        WriterHandle::spawn(state, 64)
    }

    #[tokio::test]
    async fn concurrent_appends_get_unique_monotonic_seqs() {
        let dir = TempDir::new("actor-concurrent");
        let thread = ThreadId::new();
        let writer = handle(&dir, thread);
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let writer = writer.clone();
            tasks.push(tokio::spawn(async move {
                writer.append(event_at(thread, 1)).await.unwrap().seq()
            }));
        }
        let mut seqs = Vec::new();
        for task in tasks {
            seqs.push(task.await.unwrap());
        }
        seqs.sort_unstable();
        assert_eq!(seqs, (1..=16).collect::<Vec<u64>>());
        assert_eq!(writer.watermark().await.unwrap(), 16);
    }

    #[tokio::test]
    async fn snapshot_then_append_never_loses_an_event() {
        let dir = TempDir::new("actor-no-gap");
        let thread = ThreadId::new();
        let writer = handle(&dir, thread);
        writer.append(event_at(thread, 1)).await.unwrap();

        let mut snapshot = writer.snapshot_and_subscribe(0).await.unwrap();
        assert_eq!(snapshot.watermark, 1);
        assert_eq!(snapshot.backlog.len(), 1);

        for _ in 0..5 {
            writer.append(event_at(thread, 1)).await.unwrap();
        }
        let mut seqs: Vec<u64> = snapshot.backlog.iter().map(RuntimeEvent::seq).collect();
        while let Ok(event) = snapshot.live.try_recv() {
            seqs.push(event.seq());
        }
        assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6]);
    }

    /// The requirement behind subscribe atomicity, exercised literally: write
    /// 100 events while a subscription is live and assert the concatenation of
    /// backlog and live has neither a gap nor a duplicate.
    #[tokio::test]
    async fn a_hundred_events_written_while_subscribed_have_no_gap_and_no_duplicate() {
        let dir = TempDir::new("actor-hundred");
        let thread = ThreadId::new();
        // A large live buffer keeps this test about ordering, not back-pressure:
        // the slow-client disconnect path has its own test above.
        let limits = StoreLimits {
            live_capacity: 512,
            ..StoreLimits::for_tests()
        };
        let state = WriterState::open(
            thread,
            ThreadPaths::new(dir.path().join("thread")),
            limits,
            Arc::new(MemoryDiagnostics::new()),
        )
        .unwrap();
        let writer = WriterHandle::spawn(state, 64);

        // Ten durable events first, so `subscribe` has a real backlog to return.
        for _ in 0..10 {
            writer.append(event_at(thread, 1)).await.unwrap();
        }

        let mut snapshot = writer.snapshot_and_subscribe(5).await.unwrap();
        assert_eq!(snapshot.watermark, 10);
        assert_eq!(snapshot.backlog.len(), 5, "backlog is (5, 10]");

        let producer = {
            let writer = writer.clone();
            tokio::spawn(async move {
                for _ in 0..100 {
                    writer.append(event_at(thread, 1)).await.unwrap();
                }
            })
        };

        let mut seqs: Vec<u64> = snapshot.backlog.iter().map(RuntimeEvent::seq).collect();
        while seqs.last().copied().unwrap_or(0) < 110 {
            match snapshot.live.recv().await {
                Some(event) => seqs.push(event.seq()),
                None => break,
            }
        }
        producer.await.unwrap();

        assert_eq!(
            seqs.len(),
            105,
            "5 backlog + 100 live, nothing counted twice"
        );
        assert_eq!(
            seqs,
            (6..=110).collect::<Vec<u64>>(),
            "strictly monotonic with no gap"
        );
    }

    #[tokio::test]
    async fn append_after_shutdown_reports_a_closed_writer() {
        let dir = TempDir::new("actor-shutdown");
        let thread = ThreadId::new();
        let writer = handle(&dir, thread);
        writer.shutdown().await;
        let err = writer.append(event_at(thread, 1)).await.unwrap_err();
        assert!(err.message.contains("stopped"), "{}", err.message);
    }

    #[tokio::test]
    async fn replay_reads_through_the_actor() {
        let dir = TempDir::new("actor-replay");
        let thread = ThreadId::new();
        let writer = handle(&dir, thread);
        for _ in 0..3 {
            writer.append(event_at(thread, 1)).await.unwrap();
        }
        assert_eq!(writer.replay(2).await.unwrap().len(), 1);
        assert_eq!(writer.events().await.unwrap().len(), 3);
    }
}
