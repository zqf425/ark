//! Writer state machine: sequence allocation, durable append, atomic subscribe.
//!
//! Design 7.4.3. The whole struct is owned by exactly one actor task, so every
//! method here runs inside that single serialization point. `subscribe` can
//! therefore determine the watermark, read the backlog and register the live
//! channel with no concurrent append able to slip in between them.

use crate::config::StoreLimits;
use crate::domain::diagnostics::{DiagnosticKind, DiagnosticSink, StoreDiagnostic};
use crate::domain::items::{is_delta_event, ItemProjector};
use crate::domain::layout::ThreadPaths;
use crate::domain::session_state::SessionState;
use crate::store::event_log::{
    append_text_outcome, encode_event, plan_compaction, read_markers_blocking, repair_log_blocking,
};
use crate::store::fs_util::{
    read_json_opt, remove_dir_all, sync_parent_dir, write_atomic, write_json_atomic, AppendFailure,
};
use crate::store::items_log::{
    append_item_record_blocking, append_usage_blocking, write_items_records_blocking, UsageRecord,
};
use ark_common::{ArkError, Result, ThreadId};
use ark_contract::{RewindResponse, RuntimeEvent};
use chrono::Utc;
use std::collections::{HashMap, HashSet};
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

/// Whether one event's log append must be fsynced.
///
/// Streaming deltas are recoverable by construction: a crash loses at most the
/// torn tail of `events.jsonl`, the item projection replays from the log, and
/// the client refetches the transcript instead of relying on the flushed
/// delta. Skipping the flush removes most fsyncs of a streaming turn; the
/// `fsync_on_append` limit still wins so deployments and tests can turn
/// durability off entirely.
fn append_fsync(limits: &StoreLimits, event: &RuntimeEvent) -> bool {
    limits.fsync_on_append && !is_delta_event(event)
}

#[derive(Clone, Copy)]
enum DerivedAppend {
    Item,
    Usage,
}

/// Only a row this event actually appends needs a file fsync. Session writes
/// are atomic replacements whose temporary file is already synced.
fn derived_append_fsync(event: &RuntimeEvent, target: DerivedAppend) -> bool {
    match target {
        DerivedAppend::Item => crate::domain::items::affects_items(event) && !is_delta_event(event),
        DerivedAppend::Usage => matches!(event, RuntimeEvent::Usage { .. }),
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RewindRecord {
    pub(crate) rewind_from_seq: u64,
    pub(crate) target_turn_id: Option<ark_common::TurnId>,
    pub(crate) last_seq: u64,
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
    rewinds: Vec<RewindRecord>,
    compaction_pending: bool,
    rewind_pending: bool,
    append_uncertain: bool,
    diagnostics: Arc<dyn DiagnosticSink>,
}

impl WriterState {
    /// Load the log (repairing only a torn tail) and rebuild in-memory projections.
    pub fn open(
        thread_id: ThreadId,
        paths: ThreadPaths,
        limits: StoreLimits,
        diagnostics: Arc<dyn DiagnosticSink>,
    ) -> Result<Self> {
        crate::store::compaction_tx::recover(&paths)?;
        crate::store::rewind_tx::recover(&paths)?;
        let parsed = repair_log_blocking(&paths)?;
        // Explicit gate at the writer boundary: a torn tail may be repaired,
        // but any complete malformed line means the authoritative log is
        // corrupt. Never build a divergent in-memory projection or append to it.
        parsed.reject_malformed(&paths.events_jsonl())?;
        if parsed.truncate_at.is_some() {
            diagnostics.record(
                StoreDiagnostic::new(
                    DiagnosticKind::TailRepaired,
                    "dropped an incomplete trailing event line",
                )
                .for_thread(thread_id),
            );
        }
        let (projector, session) = load_projections(&paths, &parsed.events)?;
        let rewinds = read_json_opt(&paths.rewinds_json()).unwrap_or_default();
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
            rewinds,
            compaction_pending: false,
            rewind_pending: false,
            append_uncertain: false,
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
        self.finish_pending_transactions()?;
        let seq = self.seq + 1;
        let stored = event.with_seq(seq);
        let owns_dirty =
            if crate::store::derived_repair::affects_derived(&stored) && !is_delta_event(&stored) {
                crate::store::derived_dirty::mark_if_clean(
                    &self.paths,
                    seq,
                    self.projector.current_turn(),
                    &self.session,
                )?
            } else {
                false
            };
        let mut text = encode_event(&stored)?;
        if self.needs_newline {
            // The previous tail was a complete line without a trailing newline.
            text.insert(0, '\n');
        }
        append_text_outcome(
            &self.paths.events_jsonl(),
            &text,
            append_fsync(&self.limits, &stored),
        )
        .map_err(|failure| self.handle_append_failure(owns_dirty, failure))?;
        self.needs_newline = false;
        self.seq = seq;
        self.events.push(stored.clone());
        let derived_ok = self.project_derived(&stored);
        if owns_dirty && derived_ok {
            self.clear_dirty_after_append();
        }
        self.fanout(&stored);
        Ok(stored)
    }

    fn handle_append_failure(&mut self, owns_dirty: bool, failure: AppendFailure) -> ArkError {
        if failure.rolled_back {
            if owns_dirty {
                self.clear_dirty_after_append();
            }
            return failure.error;
        }
        self.append_uncertain = true;
        let _ = crate::store::derived_dirty::mark_if_clean(
            &self.paths,
            self.seq + 1,
            self.projector.current_turn(),
            &self.session,
        );
        failure.error
    }

    /// Watermark, backlog and live channel in one indivisible step.
    ///
    /// `backlog` is the durable `(since_seq, watermark]`, therefore the live
    /// channel only ever yields `seq > watermark` and the concatenation is
    /// already ordered and gap-free.
    pub fn snapshot_and_subscribe(&mut self, since_seq: u64) -> Result<Snapshot> {
        self.finish_pending_transactions()?;
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

    /// Turn and interactive-gate lifecycle entries, in durable log order.
    pub fn interaction_events(&self) -> Vec<RuntimeEvent> {
        self.events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    RuntimeEvent::TurnStarted { .. }
                        | RuntimeEvent::TurnCompleted { .. }
                        | RuntimeEvent::TurnFailed { .. }
                        | RuntimeEvent::TurnAborted { .. }
                        | RuntimeEvent::ApprovalRequested { .. }
                        | RuntimeEvent::ApprovalResolved { .. }
                        | RuntimeEvent::UserInputRequested { .. }
                        | RuntimeEvent::UserInputResolved { .. }
                )
            })
            .cloned()
            .collect()
    }

    /// Attachment association entries only, in durable log order.
    ///
    /// Thread detail, the turn-start authorization check and the delete guard all
    /// need exactly this vocabulary. A narrow projection keeps those reads
    /// proportional to the number of associations instead of the transcript.
    pub fn attachment_events(&self) -> Vec<RuntimeEvent> {
        self.events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AttachmentAssociated { .. }))
            .cloned()
            .collect()
    }

    /// Fold the oldest events into a declared hole, keeping `keep_last` recent.
    ///
    /// Returns the last folded `seq`, i.e. where the hole ends.
    pub fn compact(&mut self, keep_last: usize) -> Result<Option<u64>> {
        self.finish_pending_transactions()?;
        self.reject_destructive_if_dirty("compact")?;
        let keep_last = self.retention_safe_keep_last(keep_last);
        let planned = plan_compaction(&self.events, keep_last, "capacity")?;
        let Some((body, marker)) = planned else {
            return Ok(None);
        };
        let cutoff = marker.to_seq;
        self.persist_compaction_checkpoints()?;
        self.compaction_pending = true;
        let transaction = crate::store::compaction_tx::CompactionTransaction::begin(
            &self.paths,
            marker,
            body.as_bytes(),
        )?;
        write_atomic(&self.paths.events_jsonl(), body.as_bytes())?;
        self.events.retain(|event| event.seq() > cutoff);
        self.needs_newline = false;
        sync_parent_dir(&self.paths.events_jsonl())?;
        transaction.commit()?;
        self.compaction_pending = false;
        Ok(Some(cutoff))
    }

    /// Physically discard every event at or after `rewind_from_seq` and append
    /// one durable reset notification in a recoverable multi-file transaction.
    pub fn rewind(
        &mut self,
        rewind_from_seq: u64,
        target_turn_id: Option<ark_common::TurnId>,
    ) -> Result<RewindResponse> {
        self.finish_pending_transactions()?;
        self.validate_rewind()?;
        let reset = RuntimeEvent::ThreadRewound {
            seq: self.seq + 1,
            thread_id: self.thread_id,
            timestamp: Utc::now(),
            rewind_from_seq,
            target_turn_id,
        };
        let mut after = self
            .events
            .iter()
            .filter(|event| event.seq() < rewind_from_seq)
            .cloned()
            .collect::<Vec<_>>();
        after.push(reset.clone());
        let body = encode_events(&after)?;
        let mut rewinds = self.rewinds.clone();
        rewinds.push(RewindRecord {
            rewind_from_seq,
            target_turn_id,
            last_seq: reset.seq(),
        });
        let transaction = crate::store::rewind_tx::RewindTransaction::begin(
            &self.paths,
            body.as_bytes(),
            &rewinds,
        )?;
        self.rewind_pending = true;
        transaction.replace_events(body.as_bytes())?;
        transaction.commit()?;
        self.install_rewound_state(after, rewinds, &reset);
        Ok(RewindResponse {
            last_seq: reset.seq(),
            rewind_from_seq,
            already_rewound: false,
        })
    }

    fn validate_rewind(&self) -> Result<()> {
        if crate::store::fs_util::path_entry_exists(&self.paths.compacted_jsonl()) {
            return Err(ArkError::conflict(format!(
                "cannot rewind compacted thread {}",
                self.thread_id
            )));
        }
        self.reject_destructive_if_dirty("rewind")?;
        if self.has_running_turn() {
            return Err(ArkError::conflict(
                "cannot rewind a thread with a running turn",
            ));
        }
        Ok(())
    }

    fn install_rewound_state(
        &mut self,
        events: Vec<RuntimeEvent>,
        rewinds: Vec<RewindRecord>,
        reset: &RuntimeEvent,
    ) {
        self.seq = reset.seq();
        self.events = events;
        self.projector = project_items_in_memory(&self.events);
        self.session = SessionState::replay(&self.events);
        self.rewinds = rewinds;
        self.needs_newline = false;
        self.rewind_pending = false;
        self.fanout(reset);
    }

    /// Find a rewind marker retained independently from the compactable event log.
    pub fn rewind_record(
        &self,
        rewind_from_seq: Option<u64>,
        target_turn_id: Option<ark_common::TurnId>,
    ) -> Option<RewindResponse> {
        self.rewinds
            .iter()
            .find(|record| {
                rewind_from_seq.is_some_and(|seq| record.rewind_from_seq == seq)
                    || target_turn_id.is_some_and(|turn| record.target_turn_id == Some(turn))
            })
            .map(|record| RewindResponse {
                last_seq: self.seq,
                rewind_from_seq: record.rewind_from_seq,
                already_rewound: true,
            })
    }

    fn has_running_turn(&self) -> bool {
        let mut running = HashSet::new();
        for event in &self.events {
            match event {
                RuntimeEvent::TurnStarted { turn_id, .. } => {
                    running.insert(*turn_id);
                }
                RuntimeEvent::TurnCompleted { turn_id, .. }
                | RuntimeEvent::TurnFailed { turn_id, .. }
                | RuntimeEvent::TurnAborted { turn_id, .. } => {
                    running.remove(turn_id);
                }
                _ => {}
            }
        }
        !running.is_empty()
    }

    /// Increases the requested retained suffix when completion or attachment
    /// semantics need an earlier boundary. An acknowledgement preceding a
    /// duplicate completion must stay with that duplicate; otherwise a later
    /// replay would turn an already-consumed notification back into an
    /// unacknowledged one.
    ///
    /// Attachment associations are pinned for the same reason: compaction
    /// rewrites the log without the folded prefix, so a folded
    /// `attachment_associated` would silently revoke an authorization that the
    /// design treats as replay-authoritative. Associations are rare and small,
    /// so keeping them all costs far less than losing one.
    fn retention_safe_keep_last(&self, keep_last: usize) -> usize {
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
            match event {
                RuntimeEvent::ChildCompleted {
                    notification_id, ..
                } => {
                    if !acknowledgements.contains_key(notification_id.as_str()) {
                        cut = cut.min(index);
                    }
                }
                RuntimeEvent::AttachmentAssociated { .. } => cut = cut.min(index),
                _ => {}
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

    /// Push derived files and report whether every required write succeeded.
    fn project_derived(&mut self, stored: &RuntimeEvent) -> bool {
        let item = self.project_item(stored);
        let usage = self.project_usage(stored);
        let session = self.project_session(stored);
        item && usage && session
    }

    fn project_item(&mut self, stored: &RuntimeEvent) -> bool {
        let Some(item_id) = self.projector.apply_in_place(stored) else {
            return true;
        };
        if is_delta_event(stored) {
            return true;
        }
        let item = self
            .projector
            .item(&item_id)
            .expect("projected item remains in the projector")
            .clone();
        let record = crate::domain::items::ItemSnapshotRecord {
            first_seq: self.projector.first_seq_of(&item_id),
            watermark_seq: self.projector.watermark_of(&item_id),
            item,
        };
        self.record_derived_result(
            "items.jsonl",
            append_item_record_blocking(
                &self.paths,
                &record,
                derived_append_fsync(stored, DerivedAppend::Item),
            ),
        )
    }

    fn project_usage(&self, stored: &RuntimeEvent) -> bool {
        let RuntimeEvent::Usage {
            turn_id,
            usage,
            seq,
            ..
        } = stored
        else {
            return true;
        };
        let record = UsageRecord::from_event(*seq, *turn_id, usage.clone());
        self.record_derived_result(
            "usage.jsonl",
            append_usage_blocking(
                &self.paths,
                &record,
                derived_append_fsync(stored, DerivedAppend::Usage),
            ),
        )
    }

    fn project_session(&mut self, stored: &RuntimeEvent) -> bool {
        if !self.session.apply(stored) {
            return true;
        }
        self.record_derived_result(
            "session.json",
            write_json_atomic(&self.paths.session_json(), &self.session),
        )
    }

    fn record_derived_result(&self, file: &str, result: Result<()>) -> bool {
        if let Err(err) = result {
            self.report_derived(file, &err);
            return false;
        }
        true
    }

    fn clear_dirty_after_append(&self) {
        if let Err(err) = crate::store::derived_dirty::clear(&self.paths) {
            self.report_derived(".derived-dirty.json", &err);
        }
    }

    fn persist_compaction_checkpoints(&self) -> Result<()> {
        let records = self.projector.clone().snapshot_records();
        write_items_records_blocking(&self.paths, &records)?;
        write_json_atomic(&self.paths.session_json(), &self.session)?;
        sync_parent_dir(&self.paths.session_json())
    }

    pub(crate) fn finish_pending_transactions(&mut self) -> Result<()> {
        if self.append_uncertain {
            return Err(ArkError::store(format!(
                "thread {} has an uncertain event append; restart the store to recover",
                self.thread_id
            )));
        }
        if self.compaction_pending {
            sync_parent_dir(&self.paths.events_jsonl())?;
            crate::store::compaction_tx::recover(&self.paths)?;
            self.compaction_pending = false;
        }
        if self.rewind_pending
            || crate::store::fs_util::path_entry_exists(&self.paths.rewind_pending_json())
        {
            let previous_seq = self.seq;
            let outcome = crate::store::rewind_tx::recover(&self.paths)?;
            self.reload_from_disk()?;
            self.rewind_pending = false;
            let committed = outcome == crate::store::rewind_tx::RecoveryOutcome::Committed
                || self.seq > previous_seq;
            if committed {
                if let Some(reset) = self
                    .events
                    .last()
                    .cloned()
                    .filter(|event| matches!(event, RuntimeEvent::ThreadRewound { .. }))
                {
                    self.fanout(&reset);
                }
            }
        }
        Ok(())
    }

    fn reload_from_disk(&mut self) -> Result<()> {
        let parsed = repair_log_blocking(&self.paths)?;
        let (projector, session) = load_projections(&self.paths, &parsed.events)?;
        self.seq = parsed.max_seq;
        self.events = parsed.events;
        self.needs_newline = parsed.needs_newline;
        self.projector = projector;
        self.session = session;
        self.rewinds = read_json_opt(&self.paths.rewinds_json()).unwrap_or_default();
        Ok(())
    }

    fn reject_destructive_if_dirty(&self, operation: &str) -> Result<()> {
        if crate::store::derived_dirty::exists(&self.paths) {
            return Err(ArkError::conflict(format!(
                "cannot {operation} thread {} while derived recovery is pending; restart the store first",
                self.thread_id
            )));
        }
        Ok(())
    }

    fn report_derived(&self, file: &str, err: &ArkError) {
        self.diagnostics.record(
            StoreDiagnostic::new(
                DiagnosticKind::DerivedWriteFailed,
                format!("derived append to {file} failed: {err}"),
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

fn load_projections(
    paths: &ThreadPaths,
    events: &[RuntimeEvent],
) -> Result<(ItemProjector, SessionState)> {
    if crate::store::compaction_tx::has_effective_history(paths) {
        let recovery = crate::store::derived_dirty::recovery_plan(paths, events, false);
        if crate::store::derived_dirty::exists(paths) && recovery.is_none() {
            return Err(ArkError::store(
                "cannot open compacted dirty thread without valid pre-append context",
            ));
        }
        let suffix = recovery
            .as_ref()
            .and_then(|plan| {
                events
                    .iter()
                    .position(|event| event.seq() == plan.planned_seq)
            })
            .map(|start| &events[start..])
            .unwrap_or(events);
        return crate::store::derived_repair::load_checkpoint_projections(
            paths,
            suffix,
            recovery.as_ref(),
        );
    }
    Ok((
        project_items_in_memory(events),
        SessionState::replay(events),
    ))
}

fn project_items_in_memory(events: &[RuntimeEvent]) -> ItemProjector {
    let mut projector = ItemProjector::new();
    for event in events {
        projector.apply_in_place(event);
    }
    projector
}

fn encode_events(events: &[RuntimeEvent]) -> Result<String> {
    let mut body = String::new();
    for event in events {
        body.push_str(&encode_event(event)?);
    }
    Ok(body)
}

#[cfg(test)]
#[path = "writer_state_tests.rs"]
mod tests;
