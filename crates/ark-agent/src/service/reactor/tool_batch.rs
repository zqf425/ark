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
        self.writer
            .append(self.events.tool_call_started(
                &synthetic.call_id,
                &synthetic.tool,
                synthetic.arguments.clone(),
            ))
            .await?;
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
        let execution = ToolExecution {
            call_id: synthetic.call_id.clone(),
            tool: synthetic.tool.clone(),
            ok: synthetic.ok,
            content: synthetic.content.clone(),
            duration_ms: 0,
            detail: Some("not executed".to_string()),
        };
        let call = ToolCallRequest::new(
            synthetic.call_id.clone(),
            synthetic.tool.clone(),
            synthetic.arguments.clone(),
        );
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
        for call in batch {
            self.writer
                .append(self.events.tool_call_started(
                    &call.call_id,
                    &call.tool,
                    call.arguments.clone(),
                ))
                .await?;
        }
        let calls = batch.to_vec();
        let tools = Arc::clone(&self.tools);
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
            results = crate::service::dispatch::run_calls(tools, calls) => results,
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
            self.append_tool_history(&call, &execution);
            executions.push(execution);
        }
        Ok((executions, false))
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
