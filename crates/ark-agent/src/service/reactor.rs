//! The reactor: **the only place in `ark-agent` that performs I/O** (design §7.1).
//!
//! It turns one [`Action`] into boundary calls on `ModelClient`, `ToolRunner`,
//! `GateBroker` and `SessionWriter`, one action at a time. It owns no decision
//! logic of its own: every "what next" answer comes from [`crate::service::step`]
//! and every counter lives in [`TurnMachine`].
//!
//! # Interrupt
//!
//! `interrupt` only calls [`CancelToken::cancel`]. The model stream and both
//! gates are awaited inside `tokio::select!` against `token.cancelled()`, so the
//! turn leaves its blocking call at the next boundary and lands on a terminal
//! `Aborted` state with a `turn_aborted` event. There is no second `aborted`
//! flag anywhere.

use std::sync::Arc;
use std::time::Instant;

use ark_common::{ArkError, ItemId, Result, Usage};
use ark_contract::{item::ItemKind, ThreadItem};
use ark_ports::{
    ModelChunk, ModelClient, ModelMessage, ModelRequest, ModelResponse, ModelRole, SandboxDecision,
    SandboxPolicy, ToolArgs, ToolOutput,
};
use tokio::sync::mpsc;

use crate::context::{HistoryHygiene, TokenEstimator};
use crate::domain::action::{Action, DispatchPlan, SyntheticResult, TurnOutcome};
use crate::domain::input::{ApprovalAsk, InputKind, TurnInput};
use crate::domain::state::TurnState;
use crate::domain::tool::{ToolCallRequest, ToolExecution};
use crate::exec::cancel::CancelToken;
use crate::exec::events::TurnEvents;
use crate::exec::gate::{ApprovalRequest, GateBroker, UserInputRequest};
use crate::exec::journal::SessionWriter;
use crate::exec::tool_runner::ToolRunner;
use crate::service::machine::TurnMachine;
use crate::service::step::step;

/// Tools that answer by asking a human, not by running.
pub const USER_INPUT_TOOL: &str = "request_user_input";

/// The static half of every request: the prefix plus the tool catalogue.
#[derive(Debug, Clone, Default)]
pub struct ModelSeed {
    pub messages: Vec<ModelMessage>,
    pub tools: Vec<serde_json::Value>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

/// Everything the reactor needs. All fields are ports, so the reactor never
/// names a concrete provider, store or tool.
pub struct ReactorConfig {
    pub model: Arc<dyn ModelClient>,
    pub tools: Arc<dyn ToolRunner>,
    pub writer: Arc<dyn SessionWriter>,
    pub gates: Arc<dyn GateBroker>,
    pub sandbox: Arc<dyn SandboxPolicy>,
    pub events: TurnEvents,
    pub cancel: CancelToken,
    pub seed: ModelSeed,
    pub machine: TurnMachine,
    /// Channel capacity for model deltas.
    pub chunk_buffer: usize,
}

impl std::fmt::Debug for ReactorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReactorConfig")
            .field("seed", &self.seed)
            .field("chunk_buffer", &self.chunk_buffer)
            .finish_non_exhaustive()
    }
}

/// One model round's result, after the stream pump folded the deltas.
enum Round {
    Settled(ModelResponse),
    Aborted,
    Failed(String),
}

/// Drives exactly one turn from `Preparing` to a terminal state.
pub struct Reactor {
    model: Arc<dyn ModelClient>,
    tools: Arc<dyn ToolRunner>,
    writer: Arc<dyn SessionWriter>,
    gates: Arc<dyn GateBroker>,
    sandbox: Arc<dyn SandboxPolicy>,
    events: TurnEvents,
    cancel: CancelToken,
    tools_schema: Vec<serde_json::Value>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    chunk_buffer: usize,
    machine: TurnMachine,
    history: Vec<ModelMessage>,
    message_item: Option<ItemId>,
    /// Calls of the current round, in model order.
    pending_calls: Vec<ToolCallRequest>,
    /// Gates not yet resolved, in model order.
    pending_approvals: Vec<ApprovalAsk>,
    /// Calls already approved during the current gate chain.
    approved: Vec<ToolCallRequest>,
    /// Synthetic results produced by earlier denials in the chain.
    denied: Vec<SyntheticResult>,
    /// Mid-turn corrections. Drained at a *round boundary* (before the next
    /// model call), never in the middle of a tool-call/result pair: injecting
    /// there would hand the provider a history it considers malformed.
    steer: Option<Arc<crate::exec::steer::SteerQueue>>,
}

impl Reactor {
    pub fn new(config: ReactorConfig) -> Self {
        let ReactorConfig {
            model,
            tools,
            writer,
            gates,
            sandbox,
            events,
            cancel,
            seed,
            machine,
            chunk_buffer,
        } = config;
        Self {
            model,
            tools,
            writer,
            gates,
            sandbox,
            events,
            cancel,
            tools_schema: seed.tools,
            temperature: seed.temperature,
            max_tokens: seed.max_tokens,
            chunk_buffer,
            machine,
            history: seed.messages,
            message_item: None,
            pending_calls: Vec::new(),
            pending_approvals: Vec::new(),
            approved: Vec::new(),
            denied: Vec::new(),
            steer: None,
        }
    }

    /// Attach a steering queue so a running turn can be corrected mid-flight.
    pub fn with_steering(mut self, queue: Arc<crate::exec::steer::SteerQueue>) -> Self {
        self.steer = Some(queue);
        self
    }

    /// Fold any queued corrections into the history as user messages.
    ///
    /// Returns how many were applied, so the caller can record a
    /// `turn_steered` event for each.
    pub fn drain_steering(&mut self) -> Vec<String> {
        let Some(queue) = &self.steer else {
            return Vec::new();
        };
        let drained = queue.drain();
        for text in &drained {
            self.history.push(ModelMessage::user(text.clone()));
        }
        drained
    }

    pub fn state(&self) -> TurnState {
        self.machine.state()
    }

    pub fn history(&self) -> &[ModelMessage] {
        &self.history
    }

    pub fn machine(&self) -> &TurnMachine {
        &self.machine
    }

    /// Append the user turn. Kept explicit so the caller controls ordering.
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.history.push(ModelMessage::user(text));
    }

    /// Run the turn. Returns the outcome actually recorded, never a guess.
    pub async fn run(&mut self) -> Result<TurnOutcome> {
        self.prepare().await?;
        let mut input = TurnInput::new(
            self.machine.limits(),
            self.machine.progress(),
            InputKind::Start,
        );
        loop {
            let action = step(self.machine.state(), &input);
            self.machine
                .enter(&action)
                .map_err(|err| ArkError::internal(err.to_string()))?;
            input = match action {
                Action::CallModel { .. } => self.round_input().await?,
                Action::Dispatch(plan) => self.dispatch(plan).await?,
                Action::RequestApproval { call, summary } => {
                    self.ask_approval(call, summary).await?
                }
                Action::AskUser { call, prompt } => self.ask_user(call, prompt).await?,
                Action::Compact { keep_recent } => self.compact(keep_recent).await?,
                Action::Finish { outcome } => return self.finish(outcome).await,
                Action::Invalid { reason } => {
                    return Err(ArkError::internal(format!("invalid turn step: {reason}")))
                }
            };
            if let Some(outcome) = self.cancelled_outcome() {
                return self.finish(outcome).await;
            }
        }
    }

    /// Emit `turn_started` and repair the history before the first round.
    async fn prepare(&mut self) -> Result<()> {
        self.writer.append(self.events.turn_started()).await?;
        let report = HistoryHygiene::new().repair(&mut self.history);
        debug_assert!(
            HistoryHygiene::new().is_legal(&self.history),
            "history must be provider-legal after repair, report: {report:?}"
        );
        Ok(())
    }

    /// A cancel that arrives between actions terminates the turn immediately.
    fn cancelled_outcome(&self) -> Option<TurnOutcome> {
        if self.cancel.is_cancelled() && !self.machine.state().is_terminal() {
            Some(TurnOutcome::aborted("turn interrupted"))
        } else {
            None
        }
    }

    fn request(&self) -> ModelRequest {
        ModelRequest {
            messages: self.history.clone(),
            tools: self.tools_schema.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
        }
    }

    fn input_of(&self, kind: InputKind) -> TurnInput {
        TurnInput::new(self.machine.limits(), self.machine.progress(), kind)
    }

    fn interrupt_input(&self, reason: &str) -> TurnInput {
        self.input_of(InputKind::Interrupt {
            reason: reason.to_string(),
        })
    }

    /// Estimate the prefix and run one model round.
    async fn round_input(&mut self) -> Result<TurnInput> {
        // Steering is applied at a *round boundary*, never mid tool-call/result
        // pair: injecting there would hand the provider a history it considers
        // malformed. `drain_steering` used to have no caller at all, so a
        // correction produced a `turn_steered` event and then never reached the
        // model (P1-6).
        self.apply_steering().await?;
        let estimate = TokenEstimator::new().estimate_request(&self.request());
        self.machine.set_prompt_estimate(estimate);
        match self.pump().await? {
            Round::Settled(response) => self.settle_model(response).await,
            Round::Aborted => Ok(self.interrupt_input("turn interrupted")),
            Round::Failed(message) => Ok(self.input_of(InputKind::ModelFailed { message })),
        }
    }

    /// Push queued corrections into the model history, then journal each one.
    ///
    /// The event is written **after** the history update so the durable log never
    /// claims a correction the model did not actually receive.
    async fn apply_steering(&mut self) -> Result<()> {
        for text in self.drain_steering() {
            self.writer.append(self.events.turn_steered(&text)).await?;
        }
        Ok(())
    }

    /// Stream one model round, folding deltas into one response.
    async fn pump(&mut self) -> Result<Round> {
        let (sender, mut receiver) = mpsc::channel(self.chunk_buffer.max(1));
        // Clone the Arc so the stream borrows a local, not `self`.
        let model = Arc::clone(&self.model);
        let stream = model.stream(self.request(), sender);
        tokio::pin!(stream);
        let mut acc = Accumulator::default();
        let mut result: Option<Result<ModelResponse>> = None;
        let mut open = true;
        while !(!open && result.is_some()) {
            tokio::select! {
                _ = self.cancel.cancelled() => return Ok(Round::Aborted),
                settled = &mut stream, if result.is_none() => {
                    result = Some(settled);
                }
                chunk = receiver.recv(), if open => match chunk {
                    Some(chunk) => self.observe(chunk, &mut acc).await?,
                    None => open = false,
                },
            }
        }
        match result {
            Some(Ok(response)) => Ok(Round::Settled(acc.finish(response))),
            Some(Err(err)) => Ok(Round::Failed(err.to_string())),
            None => Ok(Round::Failed(
                "model stream ended without a response".into(),
            )),
        }
    }

    /// Write one delta to the journal and fold it into the accumulator.
    async fn observe(&mut self, chunk: ModelChunk, acc: &mut Accumulator) -> Result<()> {
        match chunk {
            ModelChunk::TextDelta { delta } => {
                let item = self.ensure_message_item();
                self.writer
                    .append(self.events.text_delta(item, delta.clone()))
                    .await?;
                acc.text.push_str(&delta);
            }
            ModelChunk::ReasoningDelta { delta } => {
                let item = self.ensure_message_item();
                self.writer
                    .append(self.events.reasoning_delta(item, delta))
                    .await?;
            }
            ModelChunk::ToolCall {
                call_id,
                tool,
                arguments,
            } => {
                acc.calls
                    .push(ToolCallRequest::new(call_id, tool, arguments));
            }
            ModelChunk::Usage { usage } => acc.usage = Some(usage),
        }
        Ok(())
    }

    fn ensure_message_item(&mut self) -> ItemId {
        if let Some(item) = self.message_item {
            return item;
        }
        let item = ThreadItem::new(self.events.turn_id, ItemKind::AssistantMessage);
        let item_id = item.item_id;
        self.message_item = Some(item_id);
        item_id
    }

    /// Fold the settled response into history and build the `ModelSettled` input.
    async fn settle_model(&mut self, response: ModelResponse) -> Result<TurnInput> {
        let usage = response.usage.clone();
        let mut calls = Vec::new();
        for (call_id, tool, arguments) in response.tool_calls.clone() {
            // The policy is consulted **once**, here, and the decision travels
            // with the call. The executor consumes it instead of asking again,
            // which is what makes an approved `bash` actually runnable
            // (design §7.3, P1-5).
            let decision = self.sandbox.decide(&tool, &ToolArgs(arguments.clone()));
            calls.push(ToolCallRequest::new(call_id, tool, arguments).with_sandbox(decision));
        }
        self.record_assistant_turn(&response.text, &calls);
        self.machine.note_usage(&usage);
        self.writer.append(self.events.usage(usage.clone())).await?;

        self.pending_calls = calls.clone();
        self.approved.clear();
        self.denied.clear();
        self.pending_approvals = self.approval_asks(&calls);
        let repeats = self.machine.repeats_for(&calls);
        let approvals = self.pending_approvals.clone();
        Ok(self
            .input_of(InputKind::ModelSettled {
                text: response.text,
                tool_calls: calls,
                repeats,
                usage,
            })
            .with_approvals(approvals))
    }

    /// Record what the model said. `ModelMessage` has no `tool_calls` field, so
    /// the calls are rendered into the assistant text (contract gap, reported).
    fn record_assistant_turn(&mut self, text: &str, calls: &[ToolCallRequest]) {
        if calls.is_empty() {
            if !text.is_empty() {
                self.history.push(ModelMessage::assistant(text));
            }
            return;
        }
        let rendered: Vec<String> = calls
            .iter()
            .map(|call| format!("[{}({})]", call.tool, call.arguments))
            .collect();
        self.history.push(ModelMessage::assistant(format!(
            "{text}\n{}",
            rendered.join(" ")
        )));
    }

    /// Calls that need a human decision.
    ///
    /// Reads the decision already attached by [`Self::settle_model`]. The policy
    /// fallback exists only for calls constructed outside that path (synthetic
    /// or test calls) and keeps the old behaviour for them.
    fn approval_asks(&self, calls: &[ToolCallRequest]) -> Vec<ApprovalAsk> {
        calls
            .iter()
            .filter(|call| {
                let decision = call.sandbox.unwrap_or_else(|| {
                    self.sandbox
                        .decide(&call.tool, &ToolArgs(call.arguments.clone()))
                });
                decision == SandboxDecision::NeedsApproval
            })
            .map(|call| ApprovalAsk::new(call.clone(), summarize(call)))
            .collect()
    }

    /// Resolve one approval gate and return the next input.
    async fn ask_approval(&mut self, call: ToolCallRequest, summary: String) -> Result<TurnInput> {
        // `call_id` is provider-local and may repeat across concurrent turns;
        // HTTP resolution is process-wide, so scope the public gate id by turn.
        let approval_id = format!("approval_{}_{}", self.events.turn_id, call.call_id);
        self.writer
            .append(
                self.events
                    .approval_requested(&approval_id, &call.tool, &summary),
            )
            .await?;
        let gates = Arc::clone(&self.gates);
        let request = ApprovalRequest::new(approval_id.clone(), call.clone(), summary);
        let reply = tokio::select! {
            _ = self.cancel.cancelled() => {
                return Ok(self.interrupt_input("turn interrupted while awaiting approval"))
            }
            reply = gates.request_approval(request) => reply?,
        };
        self.writer
            .append(
                self.events
                    .approval_resolved(&approval_id, reply.approved()),
            )
            .await?;

        // Chain bookkeeping: the step only sees a pure snapshot of it.
        let already_approved = self.approved.clone();
        let denied_so_far = self.denied.clone();
        self.pending_approvals
            .retain(|ask| ask.call.call_id != call.call_id);
        if reply.approved() {
            self.approved.push(call.clone());
        } else {
            self.denied.push(SyntheticResult::new(
                &call,
                false,
                format!("denied by the approval gate: {}", reply.reason()),
            ));
        }
        let next_ask = self.pending_approvals.first().cloned();
        let pending_dispatch = if next_ask.is_some() {
            Vec::new()
        } else {
            already_approved
        };
        let mut aligned = pending_dispatch.clone();
        aligned.push(call.clone());
        let repeats = self.machine.repeats_for(&aligned);
        Ok(self.input_of(InputKind::GateResolved {
            call,
            approved: reply.approved(),
            reason: reply.reason(),
            next_ask,
            pending_dispatch,
            synthetic_so_far: denied_so_far,
            repeats,
        }))
    }

    /// Resolve a user-input gate. The answer becomes a tool result so the model
    /// sees it in the same turn.
    async fn ask_user(&mut self, call: ToolCallRequest, prompt: String) -> Result<TurnInput> {
        let request_id = format!("ask_{}_{}", self.events.turn_id, call.call_id);
        self.writer
            .append(self.events.user_input_requested(&request_id, &prompt))
            .await?;
        let gates = Arc::clone(&self.gates);
        let request = UserInputRequest {
            request_id: request_id.clone(),
            call: call.clone(),
            prompt,
        };
        let answer = tokio::select! {
            _ = self.cancel.cancelled() => {
                return Ok(self.interrupt_input("turn interrupted while awaiting user input"))
            }
            answer = gates.request_user_input(request) => answer?,
        };
        self.writer
            .append(self.events.user_input_resolved(&request_id, &answer))
            .await?;
        let execution = ToolExecution::ok(&call, answer.clone(), 0);
        self.append_tool_history(&call, &execution);
        self.machine.note_tool_calls(1);
        self.machine.record_calls(std::slice::from_ref(&call));
        self.writer
            .append(self.events.tool_call_finished(
                &call.call_id,
                &call.tool,
                true,
                0,
                answer,
                None,
            ))
            .await?;
        Ok(self.input_of(InputKind::ToolsSettled {
            executions: vec![execution],
        }))
    }

    /// Compact the prefix. Runs entirely in memory: the journal only records it.
    async fn compact(&mut self, keep_recent: usize) -> Result<TurnInput> {
        self.writer.append(self.events.compaction_started()).await?;
        let removed = self.compact_history(keep_recent);
        self.machine.note_compaction(removed);
        self.writer
            .append(self.events.compaction_completed(removed))
            .await?;
        Ok(self.input_of(InputKind::CompactionSettled {
            removed_items: removed,
        }))
    }

    /// Keep the system prefix and the newest messages; fold the middle away.
    fn compact_history(&mut self, keep_recent: usize) -> u32 {
        let system_count = self
            .history
            .iter()
            .take_while(|message| TokenEstimator::is_system(message))
            .count();
        let keep = keep_recent.max(1);
        if self.history.len() <= system_count + keep {
            return 0;
        }
        let split = self.history.len() - keep;
        let removed = (split - system_count) as u32;
        let mut next: Vec<ModelMessage> = self.history[..system_count].to_vec();
        next.push(ModelMessage::system(format!(
            "[{removed} earlier messages were compacted to fit the context window]"
        )));
        next.extend_from_slice(&self.history[split..]);
        self.history = next;
        removed
    }

    /// Execute a dispatch plan: synthetic results first, then batches in order.
    async fn dispatch(&mut self, plan: DispatchPlan) -> Result<TurnInput> {
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
                return Ok(self.interrupt_input("turn interrupted during tool dispatch"));
            }
        }
        self.machine.record_calls(&ran);
        self.machine.note_tool_calls(executions.len() as u32);
        Ok(self.input_of(InputKind::ToolsSettled { executions }))
    }

    /// A call that was refused: record it, then answer it anyway.
    async fn record_synthetic(
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
    async fn run_batch(&mut self, batch: &[ToolCallRequest]) -> Result<(Vec<ToolExecution>, bool)> {
        if batch.is_empty() {
            return Ok((Vec::new(), false));
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
                .append(self.events.tool_call_started(
                    &call.call_id,
                    &call.tool,
                    call.arguments.clone(),
                ))
                .await?;
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

    fn append_tool_history(&mut self, call: &ToolCallRequest, execution: &ToolExecution) {
        self.history.push(ModelMessage {
            role: ModelRole::Tool,
            content: execution.content.clone(),
            tool_call_id: Some(call.call_id.clone()),
            name: Some(call.tool.clone()),
        });
    }

    /// Write the terminal event, then land on the terminal state.
    async fn finish(&mut self, outcome: TurnOutcome) -> Result<TurnOutcome> {
        self.writer.finish(&outcome).await?;
        if self.machine.state() == TurnState::Finalizing {
            self.machine
                .mark_terminal()
                .map_err(|err| ArkError::internal(err.to_string()))?;
        }
        self.machine
            .settle(&outcome)
            .map_err(|err| ArkError::internal(err.to_string()))?;
        Ok(outcome)
    }
}

/// Render a call for the approval prompt.
fn summarize(call: &ToolCallRequest) -> String {
    let arguments = call.arguments.to_string();
    let args = ark_common::text::truncate_utf8(&arguments, 240);
    format!("{} {args}", call.tool)
}

/// Turn one raw tool result into a recorded execution.
pub(crate) fn execution_of(
    call: &ToolCallRequest,
    output: Result<ToolOutput>,
    duration_ms: u64,
) -> ToolExecution {
    match output {
        Ok(output) => {
            let mut execution = if output.ok {
                ToolExecution::ok(call, output.content, duration_ms)
            } else {
                ToolExecution::failed(call, output.content, duration_ms)
            };
            execution.detail = output.detail;
            execution
        }
        Err(err) => ToolExecution::failed(call, err.to_string(), duration_ms),
    }
}

/// Streamed state folded into a final response.
#[derive(Debug, Default)]
struct Accumulator {
    text: String,
    calls: Vec<ToolCallRequest>,
    usage: Option<Usage>,
}

impl Accumulator {
    fn finish(self, mut response: ModelResponse) -> ModelResponse {
        if response.text.is_empty() {
            response.text = self.text;
        }
        if response.tool_calls.is_empty() {
            response.tool_calls = self
                .calls
                .into_iter()
                .map(|call| (call.call_id, call.tool, call.arguments))
                .collect();
        }
        if let Some(usage) = self.usage {
            if response.usage.total_tokens == 0 {
                response.usage = usage;
            }
        }
        response
    }
}
