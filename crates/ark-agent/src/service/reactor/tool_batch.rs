//! Tool dispatch: synthetic (refused) results, then concurrent batches, then
//! the paired tool history rows the provider expects.
//!
//! Settlement is streaming (requirements/parallel-tools-v2 §1): each call's
//! `ToolCallFinished` lands as soon as the call settles, while the history rows
//! wait for the whole batch and land in submission order (§1 history rule).
//! A cancel keeps the results that already reached the settlement channel and
//! writes synthetic rows only for the calls it actually interrupted (§2).

use super::*;

impl Reactor {
    /// Execute a dispatch plan: synthetic results first, then batches in order.
    pub(super) async fn dispatch(&mut self, plan: DispatchPlan) -> Result<TurnInput> {
        let mut executions = Vec::new();
        for synthetic in &plan.synthetic_results {
            self.record_synthetic(synthetic, &mut executions).await?;
        }
        let mut ran = Vec::new();
        for batch in plan.batches {
            let (results, interrupted) = self.run_batch(&batch).await?;
            executions.extend(results);
            ran.extend(batch);
            if interrupted {
                // Remaining batches never ran: their calls sit in `ran` with no
                // journal row. Converge them so the assistant's tool_calls keep
                // paired results and resume stays provider-legal (AG-03 class).
                self.converge_pending_calls_on_cancel().await?;
                let reason = self.cancellation_reason("turn interrupted during tool dispatch");
                return Ok(self.interrupt_input(&reason));
            }
        }
        self.machine.record_calls(&ran);
        self.machine.note_tool_calls(executions.len() as u32);
        // Re-estimate now that this batch's results are in the history: the
        // step checks the compaction threshold on `ToolsSettled`, and the
        // previous round's estimate predates the very output that just grew the
        // prefix. Without this the check lags one round behind reality.
        let estimate = TokenEstimator::new().estimate_request(&self.request());
        self.machine.set_prompt_estimate(estimate);
        Ok(self.input_of(InputKind::ToolsSettled { executions }))
    }

    /// A call that was refused: record it, then answer it anyway.
    pub(super) async fn record_synthetic(
        &mut self,
        synthetic: &SyntheticResult,
        out: &mut Vec<ToolExecution>,
    ) -> Result<()> {
        let call = ToolCallRequest::new(
            synthetic.call_id.clone(),
            synthetic.tool.clone(),
            synthetic.arguments.clone(),
        );
        let mut started = false;
        let result: Result<()> = async {
            self.writer
                .append(self.events.tool_call_started(
                    &synthetic.call_id,
                    &synthetic.tool,
                    synthetic.arguments.clone(),
                ))
                .await?;
            started = true;
            if let Some(count) = synthetic.storm_count {
                // The refused call's arguments ride the suppression event: the
                // paired `ToolCallStarted` row already carries them, but a
                // suppression-scoped audit should not have to join across events.
                self.writer
                    .append(self.events.tool_storm_suppressed(
                        &synthetic.tool,
                        count,
                        synthetic.arguments.clone(),
                    ))
                    .await?;
            }
            self.writer
                .append(self.events.tool_call_finished(
                    &synthetic.call_id,
                    &synthetic.tool,
                    synthetic.ok,
                    0,
                    synthetic.content.clone(),
                    Some("not executed".to_string()),
                ))
                .await?;
            Ok(())
        }
        .await;
        if started && result.is_err() {
            // Compensation for the synthetic path: pair the Started row if the
            // writer still works, then surface the original error.
            let _ = self
                .writer
                .append(self.events.tool_call_finished(
                    &synthetic.call_id,
                    &synthetic.tool,
                    false,
                    0,
                    String::new(),
                    Some("tool event persistence failed before completion".to_string()),
                ))
                .await;
        }
        result?;
        let execution = ToolExecution {
            call_id: synthetic.call_id.clone(),
            tool: synthetic.tool.clone(),
            ok: synthetic.ok,
            content: synthetic.content.clone(),
            duration_ms: 0,
            detail: Some("not executed".to_string()),
        };
        self.append_tool_history(&call, &execution);
        out.push(execution);
        Ok(())
    }

    /// Run one batch concurrently. Returns `(executions, interrupted)`.
    pub(super) async fn run_batch(
        &mut self,
        batch: &[ToolCallRequest],
    ) -> Result<(Vec<ToolExecution>, bool)> {
        if batch.is_empty() {
            return Ok((Vec::new(), false));
        }
        // Fast path: a cancel that tripped before the batch started must not
        // launch any tool (requirements §2.1). The v1 contract also requires
        // the Started rows to be durable first (§4: started-then-cancelled
        // pairs in the journal), so write starts here before closing them.
        if self.cancel.is_cancelled() {
            let detail = "cancelled before the call completed".to_string();
            let mut executions = Vec::new();
            for call in batch {
                self.writer
                    .append(self.events.tool_call_started(
                        &call.call_id,
                        &call.tool,
                        call.arguments.clone(),
                    ))
                    .await?;
                self.writer
                    .append(self.events.tool_call_finished(
                        &call.call_id,
                        &call.tool,
                        false,
                        0,
                        String::new(),
                        Some(detail.clone()),
                    ))
                    .await?;
                let execution = ToolExecution {
                    call_id: call.call_id.clone(),
                    tool: call.tool.clone(),
                    ok: false,
                    content: String::new(),
                    duration_ms: 0,
                    detail: Some(detail.clone()),
                };
                self.append_tool_history(call, &execution);
                executions.push(execution);
            }
            return Ok((executions, true));
        }
        // One flag per submission slot: true means Started is durable but its
        // matching Finished is not. Call ids are not identities here because a
        // malformed provider response may repeat them.
        let mut open = vec![false; batch.len()];
        let result = self.run_batch_events(batch, &mut open).await;
        if let Err(err) = result {
            self.close_open_tool_calls_batch(batch, &open).await;
            return Err(err);
        }
        result
    }

    /// Place a settled call into its submission-order slot (requirements §1).
    ///
    /// Each slot fills at most once, so duplicate call ids keep both rows.
    /// The returned slot drives durable-open bookkeeping independently of id.
    fn settle_slot(
        batch: &[ToolCallRequest],
        slots: &mut [Option<ToolExecution>],
        item: crate::service::dispatch::SettledCall,
    ) -> Option<(usize, ToolExecution)> {
        let crate::service::dispatch::SettledCall {
            slot, execution, ..
        } = item;
        if slot >= batch.len() || slots[slot].is_some() {
            return None;
        }
        slots[slot] = Some(execution.clone());
        Some((slot, execution))
    }

    async fn persist_batch_starts(
        &mut self,
        batch: &[ToolCallRequest],
        open: &mut [bool],
    ) -> Result<()> {
        for (slot, call) in batch.iter().enumerate() {
            self.writer
                .append(self.events.tool_call_started(
                    &call.call_id,
                    &call.tool,
                    call.arguments.clone(),
                ))
                .await?;
            open[slot] = true;
        }
        Ok(())
    }

    async fn settle_received(
        &mut self,
        batch: &[ToolCallRequest],
        open: &mut [bool],
        slots: &mut [Option<ToolExecution>],
        item: crate::service::dispatch::SettledCall,
    ) -> Result<()> {
        let Some((slot, execution)) = Self::settle_slot(batch, slots, item) else {
            return Ok(());
        };
        self.finish_call(&execution).await?;
        open[slot] = false;
        Ok(())
    }

    async fn receive_batch_events(
        &mut self,
        batch: &[ToolCallRequest],
        open: &mut [bool],
        slots: &mut [Option<ToolExecution>],
    ) -> Result<(bool, crate::service::dispatch::SettlementStream)> {
        let calls = batch.to_vec();
        let tools = Arc::clone(&self.tools);
        let mut stream = crate::service::dispatch::run_calls_streaming(tools, calls);
        // Settle each item before awaiting the next, so cancel cannot discard a
        // result already taken from the channel. Abort drops in-flight tools;
        // the stream then drains only results that reached the channel.
        loop {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {
                    stream.abort();
                    while let Some(item) = stream.next().await {
                        self.settle_received(batch, open, slots, item).await?;
                    }
                    return Ok((true, stream));
                }
                item = stream.next() => {
                    match item {
                        Some(item) => self.settle_received(batch, open, slots, item).await?,
                        None => return Ok((false, stream)),
                    }
                }
            }
        }
    }

    async fn settle_missing_slots(
        &mut self,
        batch: &[ToolCallRequest],
        open: &mut [bool],
        slots: &mut [Option<ToolExecution>],
        detail: String,
    ) -> Result<()> {
        for (slot, missing) in slots.iter_mut().enumerate() {
            if missing.is_some() {
                continue;
            }
            let call = &batch[slot];
            let execution = ToolExecution {
                call_id: call.call_id.clone(),
                tool: call.tool.clone(),
                ok: false,
                content: String::new(),
                duration_ms: 0,
                detail: Some(detail.clone()),
            };
            self.finish_call(&execution).await?;
            open[slot] = false;
            *missing = Some(execution);
        }
        Ok(())
    }

    async fn run_batch_events(
        &mut self,
        batch: &[ToolCallRequest],
        open: &mut [bool],
    ) -> Result<(Vec<ToolExecution>, bool)> {
        self.persist_batch_starts(batch, open).await?;
        // History rows land once per batch in submission order, independently
        // of completion order and duplicate call ids.
        let mut slots: Vec<Option<ToolExecution>> = vec![None; batch.len()];
        let (cancel_hit, mut stream) = self.receive_batch_events(batch, open, &mut slots).await?;
        // A cancelled join is expected after abort; only a real panic supplies
        // the synthetic failure detail. Panic is not a turn cancellation.
        if let Err(join_error) = stream.join().await {
            let detail = format!("tool batch task failed: {join_error}");
            tracing::error!("{detail}");
            self.settle_missing_slots(batch, open, &mut slots, detail)
                .await?;
        }
        let had_gaps_before_synthetic = slots.iter().any(Option::is_none);
        if had_gaps_before_synthetic {
            self.settle_missing_slots(
                batch,
                open,
                &mut slots,
                "cancelled before the call completed".to_string(),
            )
            .await?;
        }
        debug_assert!(open.iter().all(|open| !open));
        for (call, execution) in batch.iter().zip(slots.iter()) {
            if let Some(execution) = execution {
                self.append_tool_history(call, execution);
            }
        }
        let executions: Vec<ToolExecution> = slots.into_iter().flatten().collect();
        // A cancel that found every result already settled converges normally;
        // the drive loop still observes the token afterwards.
        let interrupted = cancel_hit && had_gaps_before_synthetic;
        Ok((executions, interrupted))
    }

    /// Write one call's finished event with its execution payload.
    async fn finish_call(&mut self, execution: &ToolExecution) -> Result<()> {
        self.writer
            .append(self.events.tool_call_finished(
                &execution.call_id,
                &execution.tool,
                execution.ok,
                execution.duration_ms,
                execution.content.clone(),
                execution.detail.clone(),
            ))
            .await?;
        Ok(())
    }

    /// Best-effort compensation after a journal failure, restricted to calls
    /// still marked open. A healthy writer can still close starts that were
    /// durable before the failed append; a broken writer cannot be repaired
    /// here, so preserve the original error.
    async fn close_open_tool_calls_batch(&self, batch: &[ToolCallRequest], open: &[bool]) {
        for (slot, call) in batch.iter().enumerate() {
            if !open[slot] {
                continue;
            }
            let event = self.events.tool_call_finished(
                &call.call_id,
                &call.tool,
                false,
                0,
                String::new(),
                Some("tool event persistence failed before completion".to_string()),
            );
            // The original failure is more diagnostic than a compensation
            // failure, and the caller must still converge the turn terminal.
            let _ = self.writer.append(event).await;
        }
    }

    /// Close every call of the current round that has no tool result yet.
    /// A gate cancelled mid-wait otherwise leaves the assistant's `tool_calls`
    /// dangling with no paired `finished` row (AG-03).
    pub(super) async fn converge_pending_calls_on_cancel(&mut self) -> Result<()> {
        let mut paired: Vec<String> = self
            .history
            .iter()
            .filter(|m| m.role == ModelRole::Tool)
            .filter_map(|m| m.tool_call_id.clone())
            .collect();
        // Clone the round's calls: appending tool history needs `&mut self`,
        // so the loop must not hold an immutable borrow of `self`.
        let pending_calls = self.pending_calls.clone();
        for call in &pending_calls {
            if let Some(pos) = paired.iter().position(|id| id == &call.call_id) {
                paired.remove(pos);
                continue;
            }
            self.writer
                .append(self.events.tool_call_started(
                    &call.call_id,
                    &call.tool,
                    call.arguments.clone(),
                ))
                .await?;
            let detail = "cancelled before the call completed".to_string();
            let finished = self.writer.append(self.events.tool_call_finished(
                &call.call_id,
                &call.tool,
                false,
                0,
                String::new(),
                Some(detail.clone()),
            ));
            if let Err(err) = finished.await {
                // AG-03 compensation: the writer broke mid-close; try to pair
                // this one call before surfacing the original error.
                let _ = self
                    .writer
                    .append(self.events.tool_call_finished(
                        &call.call_id,
                        &call.tool,
                        false,
                        0,
                        String::new(),
                        Some("tool event persistence failed before completion".to_string()),
                    ))
                    .await;
                return Err(err);
            }
            let execution = ToolExecution {
                call_id: call.call_id.clone(),
                tool: call.tool.clone(),
                ok: false,
                content: String::new(),
                duration_ms: 0,
                detail: Some(detail),
            };
            self.append_tool_history(call, &execution);
        }
        Ok(())
    }

    pub(super) fn append_tool_history(
        &mut self,
        call: &ToolCallRequest,
        execution: &ToolExecution,
    ) {
        self.history.push(ModelMessage {
            role: ModelRole::Tool,
            content: execution.content.clone(),
            images: Vec::new(),
            tool_call_id: Some(call.call_id.clone()),
            name: Some(call.tool.clone()),
            tool_calls: Vec::new(),
        });
    }
}
