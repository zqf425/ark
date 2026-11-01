//! Context compaction: fold the middle of the history into one checkpoint
//! note while keeping the system prefix and the recent tail verbatim.

use super::*;

impl Reactor {
    /// Compact the prefix. Runs entirely in memory: the journal only records it.
    pub(super) async fn compact(&mut self, keep_recent: usize) -> Result<TurnInput> {
        self.writer.append(self.events.compaction_started()).await?;
        let (removed, checkpoint) = self.compact_history(keep_recent);
        if removed == 0 {
            return Ok(self.input_of(InputKind::CompactionSettled { removed_items: 0 }));
        }
        let checkpoint_chars = checkpoint
            .as_ref()
            .map_or(0, |text| text.chars().count() as u32);
        let item = ThreadItem::new(self.events.turn_id, ItemKind::Notice)
            .with_text(checkpoint.expect("removed context has a checkpoint"))
            .completed();
        self.writer.append(self.events.item_created(item)).await?;
        self.machine.note_compaction(removed);
        self.writer
            .append(self.events.compaction_completed(removed, checkpoint_chars))
            .await?;
        Ok(self.input_of(InputKind::CompactionSettled {
            removed_items: removed,
        }))
    }

    /// Keep the system prefix and the newest messages; fold the middle away.
    pub(super) fn compact_history(&mut self, keep_recent: usize) -> (u32, Option<String>) {
        let system_count = self
            .history
            .iter()
            .take_while(|message| {
                TokenEstimator::is_system(message) && !is_generated_checkpoint(message)
            })
            .count();
        let keep = keep_recent.max(1);
        if self.history.len() <= system_count + keep {
            return (0, None);
        }
        let split = self.history.len() - keep;
        let removed = (split - system_count) as u32;
        let mut next: Vec<ModelMessage> = self.history[..system_count].to_vec();
        let checkpoint =
            crate::context::ContextCheckpoint::from_messages(&self.history[system_count..split]);
        let compacted_note = format!("[{removed} earlier messages compacted]");
        let checkpoint_budget = self
            .machine
            .limits()
            .context_limit_tokens
            .saturating_sub(TokenEstimator::new().estimate_text(&compacted_note));
        let rendered = format!(
            "{}\n{compacted_note}",
            checkpoint.render_with_token_limit(checkpoint_budget)
        );
        next.push(ModelMessage::system(rendered.clone()));
        next.extend_from_slice(&self.history[split..]);
        self.history = next;
        (removed, Some(rendered))
    }
}
