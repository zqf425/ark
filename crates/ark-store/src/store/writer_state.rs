//! Writer state machine: sequence allocation, durable append, atomic subscribe.
//!
//! Design 7.4.3. The whole struct is owned by exactly one actor task, so every
//! method here runs inside that single serialization point. `subscribe` can
//! therefore determine the watermark, read the backlog and register the live
//! channel with no concurrent append able to slip in between them.

use crate::config::StoreLimits;
use crate::domain::diagnostics::{DiagnosticKind, DiagnosticSink, StoreDiagnostic};
use crate::domain::items::ItemProjector;
use crate::domain::layout::ThreadPaths;
use crate::domain::session_state::SessionState;
use crate::store::event_log::{
    append_text, encode_event, plan_compaction, read_markers_blocking, repair_log_blocking,
};
use crate::store::fs_util::{remove_dir_all, write_atomic, write_json_atomic};
use crate::store::items_log::{append_item_blocking, append_usage_blocking, UsageRecord};
use ark_common::{ArkError, Result, ThreadId};
use ark_contract::RuntimeEvent;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// One registered live subscriber. `cursor` is the last `seq` it received.
pub struct Subscriber {
    cursor: u64,
    sender: mpsc::Sender<RuntimeEvent>,
}

/// Result of the atomic snapshot-then-subscribe operation.
pub struct Snapshot {
    pub backlog: Vec<RuntimeEvent>,
    pub watermark: u64,
    pub live: mpsc::Receiver<RuntimeEvent>,
}

/// Mutable state owned exclusively by one writer actor.
pub struct WriterState {
    thread_id: ThreadId,
    paths: ThreadPaths,
    seq: u64,
    events: Vec<RuntimeEvent>,
    subscribers: Vec<Subscriber>,
    limits: StoreLimits,
    needs_newline: bool,
    projector: ItemProjector,
    session: SessionState,
    diagnostics: Arc<dyn DiagnosticSink>,
}

impl WriterState {
    /// Load the log (repairing a torn tail) and rebuild the derived projections.
    pub fn open(
        thread_id: ThreadId,
        paths: ThreadPaths,
        limits: StoreLimits,
        diagnostics: Arc<dyn DiagnosticSink>,
    ) -> Result<Self> {
        let parsed = repair_log_blocking(&paths)?;
        if parsed.truncate_at.is_some() {
            diagnostics.record(
                StoreDiagnostic::new(
                    DiagnosticKind::TailRepaired,
                    "dropped an incomplete trailing event line",
                )
                .for_thread(thread_id),
            );
        }
        let mut projector = ItemProjector::new();
        for event in &parsed.events {
            projector.apply(event);
        }
        let session = SessionState::replay(&parsed.events);
        Ok(Self {
            thread_id,
            paths,
            seq: parsed.max_seq,
            needs_newline: parsed.needs_newline,
            events: parsed.events,
            subscribers: Vec::new(),
            limits,
            projector,
            session,
            diagnostics,
        })
    }

    /// Highest durable `seq` (0 for an empty log).
    pub fn watermark(&self) -> u64 {
        self.seq
    }

    pub fn paths(&self) -> &ThreadPaths {
        &self.paths
    }

    pub fn events(&self) -> &[RuntimeEvent] {
        &self.events
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    /// Assign the next `seq`, append and flush, then fan out.
    ///
    /// The in-memory watermark only advances after the append succeeded, so a
    /// failed write cannot hand out a `seq` that is not on disk.
    pub fn append(&mut self, event: RuntimeEvent) -> Result<RuntimeEvent> {
        let seq = self.seq + 1;
        let stored = event.with_seq(seq);
        let mut text = encode_event(&stored)?;
        if self.needs_newline {
            // The previous tail was a complete line without a trailing newline.
            text.insert(0, '\n');
        }
        append_text(
            &self.paths.events_jsonl(),
            &text,
            self.limits.fsync_on_append,
        )?;
        self.needs_newline = false;
        self.seq = seq;
        self.events.push(stored.clone());
        self.project_derived(&stored);
        self.fanout(&stored);
        Ok(stored)
    }

    /// Watermark, backlog and live channel in one indivisible step.
    ///
    /// `backlog` is the durable `(since_seq, watermark]`, therefore the live
    /// channel only ever yields `seq > watermark` and the concatenation is
    /// already ordered and gap-free.
    pub fn snapshot_and_subscribe(&mut self, since_seq: u64) -> Result<Snapshot> {
        let watermark = self.seq;
        let backlog = self.backlog(since_seq, watermark)?;
        let (sender, receiver) = mpsc::channel(self.limits.live_capacity);
        self.subscribers.push(Subscriber {
            cursor: watermark,
            sender,
        });
        Ok(Snapshot {
            backlog,
            watermark,
            live: receiver,
        })
    }

    /// Durable events with `seq > since_seq`, in ascending `seq` order.
    pub fn replay(&self, since_seq: u64) -> Vec<RuntimeEvent> {
        self.events
            .iter()
            .filter(|event| event.seq() > since_seq)
            .cloned()
            .collect()
    }

    /// Completion lifecycle entries only, in durable log order.
    ///
    /// This intentionally scans the actor-owned log instead of cloning every
    /// event. The callers need only these two event kinds to build the inbox
    /// and the bounded child-completion view.
    ///
    /// Returns: cloned `child_completed` and `child_completion_acknowledged`
    /// records, ordered by their durable sequence.
    pub fn child_completion_events(&self) -> Vec<RuntimeEvent> {
        self.events
            .iter()
            .filter(|event| event.is_child_completion_lifecycle())
            .cloned()
            .collect()
    }

    /// Fold the oldest events into a declared hole, keeping `keep_last` recent.
    ///
    /// Returns the last folded `seq`, i.e. where the hole ends.
    pub fn compact(&mut self, keep_last: usize) -> Result<Option<u64>> {
        let keep_last = self.completion_safe_keep_last(keep_last);
        let planned = plan_compaction(&self.events, keep_last, "capacity")?;
        let Some((body, marker)) = planned else {
            return Ok(None);
        };
        write_atomic(&self.paths.events_jsonl(), body.as_bytes())?;
        append_text(
            &self.paths.compacted_jsonl(),
            &format!("{}\n", marker.to_line()),
            self.limits.fsync_on_append,
        )?;
        let cutoff = marker.to_seq;
        self.events.retain(|event| event.seq() > cutoff);
        Ok(Some(cutoff))
    }

    /// Increases the requested retained suffix when completion semantics need
    /// an earlier boundary. An acknowledgement preceding a duplicate
    /// completion must stay with that duplicate; otherwise a later replay
    /// would turn an already-consumed notification back into an unacknowledged
    /// one.
    fn completion_safe_keep_last(&self, keep_last: usize) -> usize {
        let acknowledgements: HashMap<&str, usize> = self
            .events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| match event {
                RuntimeEvent::ChildCompletionAcknowledged {
                    notification_id, ..
                } => Some((notification_id.as_str(), index)),
                _ => None,
            })
            .fold(HashMap::new(), |mut indexes, (notification_id, index)| {
                indexes.entry(notification_id).or_insert(index);
                indexes
            });
        let mut cut = self.events.len().saturating_sub(keep_last.max(1));
        for (index, event) in self.events.iter().enumerate() {
            if let RuntimeEvent::ChildCompleted {
                notification_id, ..
            } = event
            {
                if !acknowledgements.contains_key(notification_id.as_str()) {
                    cut = cut.min(index);
                }
            }
        }

        let mut index = self.events.len();
        while index > cut {
            index -= 1;
            if let RuntimeEvent::ChildCompleted {
                notification_id, ..
            } = &self.events[index]
            {
                if let Some(ack_index) = acknowledgements.get(notification_id.as_str()) {
                    cut = cut.min(*ack_index);
                }
            }
        }
        self.events.len() - cut
    }

    /// Compaction markers currently declared for this thread.
    pub fn markers(&self) -> Vec<crate::domain::compaction::CompactionMarker> {
        read_markers_blocking(&self.paths)
    }

    /// Remove the whole thread directory (delete / orphan cleanup path).
    pub fn remove_dir(&self) -> Result<()> {
        remove_dir_all(self.paths.dir())
    }

    fn backlog(&self, since_seq: u64, watermark: u64) -> Result<Vec<RuntimeEvent>> {
        let backlog: Vec<RuntimeEvent> = self.replay(since_seq);
        if backlog.len() > self.limits.max_backlog_events {
            return Err(ArkError::store(format!(
                "backlog of {} events exceeds the {} event limit for thread {}",
                backlog.len(),
                self.limits.max_backlog_events,
                self.thread_id
            ))
            .with_detail(format!("watermark={watermark} since_seq={since_seq}")));
        }
        Ok(backlog)
    }

    /// Push derived files. They are projections, so failures are diagnostics
    /// rather than errors: the event log remains authoritative.
    fn project_derived(&mut self, stored: &RuntimeEvent) {
        if let Some(item) = self.projector.apply(stored) {
            if let Err(err) = append_item_blocking(&self.paths, &item, false) {
                self.report_derived("items.jsonl", &err);
            }
        }
        if let RuntimeEvent::Usage {
            turn_id,
            usage,
            seq,
            ..
        } = stored
        {
            let record = UsageRecord::from_event(*seq, *turn_id, usage.clone());
            if let Err(err) = append_usage_blocking(&self.paths, &record, false) {
                self.report_derived("usage.jsonl", &err);
            }
        }
        if self.session.apply(stored) {
            if let Err(err) = write_json_atomic(&self.paths.session_json(), &self.session) {
                self.report_derived("session.json", &err);
            }
        }
    }

    fn report_derived(&self, file: &str, err: &ArkError) {
        self.diagnostics.record(
            StoreDiagnostic::new(
                DiagnosticKind::ItemsRebuilt,
                format!("derived write to {file} failed, it will be rebuilt: {err}"),
            )
            .for_thread(self.thread_id),
        );
    }

    /// Non-blocking fan-out. A full channel drops that subscriber so the
    /// persistence path is never back-pressured by a slow client.
    fn fanout(&mut self, stored: &RuntimeEvent) {
        let mut dropped = 0usize;
        let seq = stored.seq();
        self.subscribers.retain_mut(|subscriber| {
            if subscriber.cursor >= seq {
                return true;
            }
            match subscriber.sender.try_send(stored.clone()) {
                Ok(()) => {
                    subscriber.cursor = seq;
                    true
                }
                Err(_) => {
                    dropped += 1;
                    false
                }
            }
        });
        if dropped > 0 {
            self.diagnostics.record(
                StoreDiagnostic::new(
                    DiagnosticKind::SlowSubscriberDropped,
                    format!("dropped {dropped} slow subscriber(s) at seq {seq}"),
                )
                .for_thread(self.thread_id),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::diagnostics::MemoryDiagnostics;
    use crate::store::tests::{event_at, TempDir};
    use ark_common::ChildId;
    use chrono::Utc;

    fn state(dir: &TempDir, thread: ThreadId) -> WriterState {
        WriterState::open(
            thread,
            ThreadPaths::new(dir.path().join("thread")),
            StoreLimits::for_tests(),
            Arc::new(MemoryDiagnostics::new()),
        )
        .unwrap()
    }

    fn child_completed(thread: ThreadId, notification_id: &str) -> RuntimeEvent {
        RuntimeEvent::ChildCompleted {
            seq: 0,
            thread_id: thread,
            timestamp: Utc::now(),
            notification_id: notification_id.to_string(),
            child_id: ChildId::new(),
            label: "inspect runtime".to_string(),
            status: "completed".to_string(),
            summary: Some("healthy".to_string()),
            error: None,
            usage: None,
            tool_invocations: None,
        }
    }

    #[test]
    fn compaction_keeps_an_unacknowledged_child_completion() {
        let dir = TempDir::new("completion-compaction");
        let thread = ThreadId::new();
        let mut state = state(&dir, thread);
        state.append(event_at(thread, 1)).unwrap();
        state
            .append(child_completed(thread, "child:pending"))
            .unwrap();
        for _ in 0..3 {
            state.append(event_at(thread, 1)).unwrap();
        }

        let compacted_to = state.compact(2).unwrap();

        assert_eq!(compacted_to, Some(1));
        assert!(state.events().iter().any(|event| {
            matches!(
                event,
                RuntimeEvent::ChildCompleted { notification_id, .. }
                    if notification_id == "child:pending"
            )
        }));
    }

    #[test]
    fn completion_query_excludes_unrelated_event_payloads() {
        let dir = TempDir::new("completion-query");
        let thread = ThreadId::new();
        let mut state = state(&dir, thread);
        state.append(event_at(thread, 1)).unwrap();
        state.append(child_completed(thread, "child:one")).unwrap();

        let events = state.child_completion_events();

        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], RuntimeEvent::ChildCompleted { .. }));
    }

    #[test]
    fn acknowledged_child_completion_no_longer_blocks_compaction() {
        let dir = TempDir::new("acknowledged-completion-compaction");
        let thread = ThreadId::new();
        let mut state = state(&dir, thread);
        state.append(event_at(thread, 1)).unwrap();
        state
            .append(child_completed(thread, "child:acknowledged"))
            .unwrap();
        state
            .append(RuntimeEvent::ChildCompletionAcknowledged {
                seq: 0,
                thread_id: thread,
                turn_id: ark_common::TurnId::new(),
                timestamp: Utc::now(),
                notification_id: "child:acknowledged".to_string(),
            })
            .unwrap();
        for _ in 0..3 {
            state.append(event_at(thread, 1)).unwrap();
        }

        let compacted_to = state.compact(2).unwrap();

        assert_eq!(compacted_to, Some(4));
        assert!(state
            .events()
            .iter()
            .all(|event| !event.is_child_completion_lifecycle()));
    }

    #[test]
    fn compaction_keeps_an_acknowledgement_for_a_retained_duplicate_completion() {
        let dir = TempDir::new("duplicate-completion-compaction");
        let thread = ThreadId::new();
        let mut state = state(&dir, thread);
        state.append(event_at(thread, 1)).unwrap();
        state
            .append(child_completed(thread, "child:duplicate"))
            .unwrap();
        state
            .append(RuntimeEvent::ChildCompletionAcknowledged {
                seq: 0,
                thread_id: thread,
                turn_id: ark_common::TurnId::new(),
                timestamp: Utc::now(),
                notification_id: "child:duplicate".to_string(),
            })
            .unwrap();
        state
            .append(child_completed(thread, "child:duplicate"))
            .unwrap();
        state.append(event_at(thread, 1)).unwrap();
        state.append(event_at(thread, 1)).unwrap();

        let compacted_to = state.compact(3).unwrap();

        assert_eq!(compacted_to, Some(2));
        assert!(state.events().iter().any(|event| {
            matches!(
                event,
                RuntimeEvent::ChildCompletionAcknowledged { notification_id, .. }
                    if notification_id == "child:duplicate"
            )
        }));
    }
}
