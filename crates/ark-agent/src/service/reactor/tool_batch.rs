//! Tool dispatch: synthetic (refused) results, then concurrent batches, then
//! the paired tool history rows the provider expects.
//!
//! Settlement is streaming (requirements/parallel-tools-v2 §1): each call's
//! `ToolCallFinished` lands as soon as the call settles, while the history rows
//! wait for the whole batch and land in submission order (§1 history rule).
//! A cancel keeps the results that already reached the settlement channel and
//! writes synthetic rows only for the calls it actually interrupted (§2).

use std::collections::BTreeMap;

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
        let mut open: BTreeMap<String, &ToolCallRequest> = batch
            .iter()
            .map(|call| (call.call_id.clone(), call))
            .collect();
        let result = self.run_batch_events(batch, &mut open).await;
        if let Err(err) = result {
            self.close_open_tool_calls_batch(batch, &open).await;
            return Err(err);
        }
        result
    }

    /// Place a settled call into its submission-order slot (requirements §1).
    ///
    /// Consuming match: each slot fills at most once, so a duplicate call id
    /// keeps both rows instead of letting the second result overwrite the
    /// first. Returns `(slot, execution)` on success, `None` when the result's
    /// call id matches no open slot of this batch (a stray id can only come
    /// from a malformed runner).
    fn settle_slot(
        batch: &[ToolCallRequest],
        slots: &mut [Option<ToolExecution>],
        item: crate::service::dispatch::SettledCall,
    ) -> Option<ToolExecution> {
        let crate::service::dispatch::SettledCall {
            slot, execution, ..
        } = item;
        if slot >= batch.len() || slots[slot].is_some() {
            return None;
        }
        slots[slot] = Some(execution.clone());
        Some(execution)
    }

    async fn run_batch_events<'a>(
        &mut self,
        batch: &'a [ToolCallRequest],
        open: &mut BTreeMap<String, &'a ToolCallRequest>,
    ) -> Result<(Vec<ToolExecution>, bool)> {
        for call in batch {
            self.writer
                .append(self.events.tool_call_started(
                    &call.call_id,
                    &call.tool,
                    call.arguments.clone(),
                ))
                .await?;
        }
        // History rows land once per batch, in submission order, from slots.
        // `slots[i]` holds the settled execution of `batch[i]`, or `None` while
        // the call is still in flight (§1 history rule: the model's ordering
        // of its own tool results is the only order the provider history may
        // show, and duplicate call ids each keep their own slot).
        let mut slots: Vec<Option<ToolExecution>> = vec![None; batch.len()];
        let mut interrupted = false;

        let calls = batch.to_vec();
        let tools = Arc::clone(&self.tools);
        let mut stream = crate::service::dispatch::run_calls_streaming(tools, calls);
        // Per-item select (requirements §2.2): each received call is settled
        // *before* awaiting the next one, so no settled result ever lives in a
        // future that a cancel would drop. On cancel: abort the forwarder —
        // dropping the join_all tree drops every in-flight tool future, and a
        // bash's kill_on_drop guard kills the child process, the v1 semantics
        // a detached task would silently break — then drain what already
        // reached the channel. No wait window: the boundary is the channel,
        // not time (§2.3).
        loop {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {
                    stream.abort();
                    while let Some(item) = stream.next().await {
                        if let Some(slot_exec) = Self::settle_slot(batch, &mut slots, item) {
                            self.finish_call(&slot_exec).await?;
                            open.remove(&slot_exec.call_id);
                        }
                    }
                    interrupted = true;
                    break;
                }
                item = stream.next() => {
                    match item {
                        Some(item) => {
                            if let Some(slot_exec) = Self::settle_slot(batch, &mut slots, item) {
                                self.finish_call(&slot_exec).await?;
                                open.remove(&slot_exec.call_id);
                            }
                        }
                        // Stream exhausted: every call settled.
                        None => break,
                    }
                }
            }
        }
        // A panic inside the forwarder is invisible to the channel (the task
        // dies without closing through join_all), so ask the join handle: a
        // panic means every not-yet-settled call lost its result and needs a
        // synthetic row (§2.5).
        if stream.join().await.is_err() {
            tracing::error!("tool batch task failed: forwarder panicked");
            interrupted = true;
        }
        if slots.iter().any(Option::is_none) {
            // Cancel or panic: settled calls keep their real result (§2.3);
            // the never-settled rest is synthetic (§2.4/§2.5).
            let cancelled = "cancelled before the call completed".to_string();
            let panicked = "tool batch task failed: forwarder panicked".to_string();
            let synthetic_detail = if interrupted { cancelled } else { panicked };
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
                    detail: Some(synthetic_detail.clone()),
                };
                self.finish_call(&execution).await?;
                open.remove(&call.call_id);
                *missing = Some(execution);
            }
        }
        // All open calls must now be settled: every slot got a real or a
        // synthetic execution, so the journal shows no dangling start. Verify
        // via the map instead of assuming, because a duplicate call id shares
        // its map key across slots.
        if !open.is_empty() {
            self.close_open_tool_calls_batch(batch, open).await;
        }
        // History rows in submission order (§1); the consumer-visible history
        // stays byte-identical to v1's for a fully-completed batch.
        for (call, execution) in batch.iter().zip(slots.iter()) {
            if let Some(execution) = execution {
                self.append_tool_history(call, execution);
            }
        }
        let executions: Vec<ToolExecution> = slots.into_iter().flatten().collect();
        // When the whole batch was already settled when cancel fired, converge
        // to the normal completion path (§2.4): the drive loop's terminal
        // handling still observes the cancel and ends the turn.
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
    async fn close_open_tool_calls_batch(
        &self,
        batch: &[ToolCallRequest],
        open: &BTreeMap<String, &ToolCallRequest>,
    ) {
        for call in batch {
            if !open.contains_key(&call.call_id) {
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
