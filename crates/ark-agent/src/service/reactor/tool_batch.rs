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
                serde_json::Value::Null,
            ))
            .await?;
        if let Some(count) = synthetic.storm_count {
            self.writer
                .append(self.events.tool_storm_suppressed(&synthetic.tool, count))
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
            serde_json::Value::Null,
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
            _ = self.cancel.cancelled() => return Ok((Vec::new(), true)),
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
            tool_call_id: Some(call.call_id.clone()),
            name: Some(call.tool.clone()),
            tool_calls: Vec::new(),
        });
    }
}
