//! Tool dispatch: synthetic (refused) results, then concurrent batches, then
//! the paired tool history rows the provider expects.

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
            self.close_open_tool_calls(&[&call]).await;
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
        let mut open = Vec::new();
        let result = self.run_batch_events(batch, &mut open).await;
        if let Err(err) = result {
            self.close_open_tool_calls(&open).await;
            return Err(err);
        }
        result
    }

    async fn run_batch_events<'a>(
        &mut self,
        batch: &'a [ToolCallRequest],
        open: &mut Vec<&'a ToolCallRequest>,
    ) -> Result<(Vec<ToolExecution>, bool)> {
        for call in batch {
            self.writer
                .append(self.events.tool_call_started(
                    &call.call_id,
                    &call.tool,
                    call.arguments.clone(),
                ))
                .await?;
            open.push(call);
        }
        let calls = batch.to_vec();
        let tools = Arc::clone(&self.tools);
        // Use concurrency-limited execution with default limit of 8
        let semaphore = Arc::new(tokio::sync::Semaphore::new(8));
        let results = tokio::select! {
            _ = self.cancel.cancelled() => {
                // Every started call must get its paired `finished` row before
                // the turn unwinds: started-without-finished reads as a hung
                // execution in the journal and ark-store, and the assistant
                // row already carries these call ids, so the history needs
                // result rows to stay provider-legal.
                for call in batch {
                    let detail = "cancelled before the call completed".to_string();
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
                    open.remove(0);
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
                return Ok((Vec::new(), true));
            }
            results = crate::service::dispatch::run_calls_with_limit(tools, calls, semaphore) => results,
        };
        let mut executions = Vec::new();
        for execution in results {
            let call = batch
                .iter()
                .find(|call| call.call_id == execution.call_id)
                .cloned()
                .unwrap_or_else(|| {
                    ToolCallRequest::new(
                        execution.call_id.clone(),
                        execution.tool.clone(),
                        serde_json::Value::Null,
                    )
                });
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
            open.remove(0);
            self.append_tool_history(&call, &execution);
            executions.push(execution);
        }
        Ok((executions, false))
    }

    /// Best-effort compensation after a journal failure. A healthy writer can
    /// still close starts that were durable before the failed append; a broken
    /// writer cannot be repaired here, so preserve the original error.
    async fn close_open_tool_calls(&self, open: &[&ToolCallRequest]) {
        for call in open {
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
                self.close_open_tool_calls(&[call]).await;
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
