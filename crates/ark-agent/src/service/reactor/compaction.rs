//! Context compaction: fold the middle of the history into one checkpoint
//! note while keeping the system prefix and the recent tail verbatim.

use super::*;
use crate::context::checkpoint::retained_tail_fingerprint;

/// Prefix of the in-memory plan restate row pushed by `compact_history`.
/// Marker identity for the retain that deduplicates restates across repeated
/// compactions; the restate is never journalled, so this prefix is only ever
/// matched inside the live history. The trailing newline is part of the
/// marker on purpose: the turn-seed header is
/// `[thread plan - current objective and progress]`, which does NOT match —
/// the retain must never delete a seed row that `keep_recent` kept in the
/// retained window (review M1). The restate render always emits
/// `[thread plan]\n<rows>`, so a header-less line cannot exist.
pub(super) const PLAN_RESTATE_PREFIX: &str = "[thread plan]\n";

impl Reactor {
    /// Compact the prefix. The in-memory history is replaced immediately; the
    /// journal records a checkpoint item that becomes the durable boundary for
    /// the next turn (`arkd::wiring::history::conversation_history`).
    pub(super) async fn compact(&mut self, keep_recent: usize) -> Result<TurnInput> {
        self.writer.append(self.events.compaction_started()).await?;
        let (removed, checkpoint) = self.compact_history(keep_recent);
        // Measure the *shortened* history so the budget can decide whether one
        // more compaction is needed. Only a fold that left the prefix above the
        // threshold spends the retry budget: that is the case that can loop
        // (nothing foldable is left, or the kept tail alone overflows), while a
        // fold that brought the prefix back under the limit is the threshold
        // working as designed and may repeat as the turn grows.
        let estimator = TokenEstimator::new();
        let estimate = estimator.estimate_request(&self.request());
        self.machine.set_prompt_estimate(estimate);
        if estimate >= self.machine.limits().context_limit_tokens {
            self.machine.note_compaction(removed);
        }
        if removed == 0 {
            return Ok(self.input_of(InputKind::CompactionSettled { removed_items: 0 }));
        }
        let checkpoint_chars = checkpoint
            .as_ref()
            .map_or(0, |text| text.chars().count() as u32);
        let item = ThreadItem::new(self.events.turn_id, ItemKind::Notice)
            .with_text(checkpoint.expect("removed context has a checkpoint"))
            .with_marker(ark_contract::item::ItemMarker::ContextCheckpoint)
            .completed();
        self.writer.append(self.events.item_created(item)).await?;
        self.writer
            .append(self.events.compaction_completed(removed, checkpoint_chars))
            .await?;
        Ok(self.input_of(InputKind::CompactionSettled {
            removed_items: removed,
        }))
    }

    /// Keep every non-generated system row, then fold the oldest dialogue under
    /// a complete request budget. The durable checkpoint includes the retained
    /// tail because the next turn replays only rows after its boundary.
    pub(super) fn compact_history(&mut self, keep_recent: usize) -> (u32, Option<String>) {
        let parts = partition_history(&self.history);
        if parts.foldable.len() < 2 {
            return (0, None);
        }
        let checkpoint = crate::context::ContextCheckpoint::from_compaction_history(&self.history);
        let plan = checkpoint.plan_restate();
        let estimator = TokenEstimator::new();
        let mut fixed_tokens = estimator.estimate_request(&self.fixed_request(&parts.protected));
        if let Some(plan) = &plan {
            fixed_tokens += estimator.estimate_message(&ModelMessage::system(plan.clone()));
        }
        let target = self
            .machine
            .limits()
            .context_limit_tokens
            .saturating_sub(COMPACTION_SAFETY_TOKENS);
        let note_reserve = estimator.estimate_text(&checkpoint_bookkeeping(
            parts.foldable.len(),
            parts.foldable.len(),
            0,
        ));
        let checkpoint_budget = target.saturating_sub(fixed_tokens + note_reserve + 4);
        let checkpoint_text = checkpoint.render_with_token_limit(checkpoint_budget);
        let checkpoint_cost = estimator.estimate_message(&ModelMessage::system(format!(
            "{checkpoint_text}\n{}",
            checkpoint_bookkeeping(parts.foldable.len(), parts.foldable.len(), 0)
        )));
        let tail_budget = target.saturating_sub(fixed_tokens + checkpoint_cost);
        let Some(retained) =
            legal_tail_count(&parts.foldable, keep_recent.max(1), tail_budget, &estimator)
        else {
            return (0, None);
        };
        let removed = parts.foldable.len() - retained;
        let rendered = format!(
            "{checkpoint_text}\n{}",
            checkpoint_bookkeeping(
                removed,
                retained,
                retained_tail_fingerprint(&parts.foldable[parts.foldable.len() - retained..])
            )
        );
        self.history = parts.protected;
        self.history.push(ModelMessage::system(rendered.clone()));
        self.history
            .extend_from_slice(&parts.foldable[parts.foldable.len() - retained..]);
        if let Some(plan) = plan {
            self.history.push(ModelMessage::system(plan));
        }
        (removed as u32, Some(rendered))
    }

    fn fixed_request(&self, protected: &[ModelMessage]) -> ModelRequest {
        let mut request = self.request();
        let staged = request.messages.split_off(self.history.len());
        request.messages = protected.to_vec();
        request.messages.extend(staged);
        request
    }
}

const COMPACTION_SAFETY_TOKENS: u64 = 1_024;

fn checkpoint_bookkeeping(removed: usize, retained: usize, fingerprint: u64) -> String {
    format!(
        "[{removed} earlier messages compacted]\n[retained tail already summarized: {retained}; fingerprint: {fingerprint:016x}]"
    )
}

struct CompactionParts {
    protected: Vec<ModelMessage>,
    foldable: Vec<ModelMessage>,
}

fn partition_history(history: &[ModelMessage]) -> CompactionParts {
    let checkpoint_start = history
        .iter()
        .rposition(is_generated_checkpoint)
        .map_or(0, |index| index + 1);
    let mut parts = CompactionParts {
        protected: history[..checkpoint_start]
            .iter()
            .filter(|message| {
                message.role == ModelRole::System
                    && !is_generated_checkpoint(message)
                    && !message.content.starts_with(PLAN_RESTATE_PREFIX)
            })
            .cloned()
            .collect(),
        foldable: Vec::new(),
    };
    for message in &history[checkpoint_start..] {
        if is_generated_checkpoint(message) {
            continue;
        }
        if message.role == ModelRole::System {
            if !message.content.starts_with(PLAN_RESTATE_PREFIX) {
                parts.protected.push(message.clone());
            }
        } else {
            parts.foldable.push(message.clone());
        }
    }
    parts
}

fn legal_tail_count(
    messages: &[ModelMessage],
    requested: usize,
    budget: u64,
    estimator: &TokenEstimator,
) -> Option<usize> {
    let maximum = requested.min(messages.len().saturating_sub(1));
    for count in (1..=maximum).rev() {
        let mut start = messages.len() - count;
        while start > 0 && messages[start].role == ModelRole::Tool {
            start -= 1;
        }
        if start == 0 {
            continue;
        }
        let count = messages.len() - start;
        let cost: u64 = messages[start..]
            .iter()
            .map(|message| estimator.estimate_message(message))
            .sum();
        if cost <= budget {
            return Some(count);
        }
    }
    None
}
