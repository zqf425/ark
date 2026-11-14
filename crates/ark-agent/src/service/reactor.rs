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

use ark_common::{ArkError, AttachmentId, ItemId, Result, Usage};
use ark_contract::{item::ItemKind, ThreadItem, TurnAttachmentOutcome};
use ark_ports::{
    CancelToken, ModelChunk, ModelClient, ModelImage, ModelMessage, ModelRequest, ModelResponse,
    ModelRole, SandboxDecision, SandboxPolicy, SteerEntry, SteerQueue, ToolArgs, ToolCallRef,
    ToolOutput,
};
use tokio::sync::mpsc;

use crate::context::checkpoint::is_generated_checkpoint;
use crate::context::{HistoryHygiene, TokenEstimator};
use crate::domain::action::{Action, DispatchPlan, SyntheticResult, TurnOutcome};
use crate::domain::input::{ApprovalAsk, InputKind, TurnInput, UserInputAsk};
use crate::domain::state::TurnState;
use crate::domain::tool::{ToolCallRequest, ToolExecution};
use crate::exec::events::TurnEvents;
use crate::exec::gate::{ApprovalRequest, GateBroker, UserInputRequest};
use crate::exec::journal::SessionWriter;
use crate::exec::tool_runner::ToolRunner;
use crate::policy::budget::{BudgetVerdict, TurnBudget};
use crate::service::machine::TurnMachine;
use crate::service::step::step;

mod compaction;
mod gate;
mod tool_batch;

/// Tools that answer by asking a human, not by running.
pub const USER_INPUT_TOOL: &str = "request_user_input";

/// The static half of every request: the prefix plus the tool catalogue.
#[derive(Debug, Clone, Default)]
pub struct ModelSeed {
    pub messages: Vec<ModelMessage>,
    pub tools: Vec<serde_json::Value>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub reasoning_effort: Option<String>,
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
    /// Resolves the attachment ids a steer was queued with into image parts at
    /// round-build time. `None` keeps steers text-only; the composition root
    /// injects a resolver built on its attachment store (with the same vision
    /// gate and per-turn image budget as a turn start), so the reactor never
    /// touches the filesystem.
    pub steer_image_resolver: Option<SteerImageResolver>,
    /// Records the delivery outcomes of a steered correction's attachments for
    /// the durable item, mirroring the seed-time rules. `None` keeps steered
    /// items bare (tests, runtimes without attachments).
    pub steer_outcome_recorder: Option<SteerOutcomeRecorder>,
}

/// Synchronous resolver the composition root installs so staged steering
/// corrections can carry image parts. `ids` are the ids drained with the
/// round's corrections; the result is one image part per id that resolved
/// (degraded/skipped ones simply produce no part, matching seed-time rules).
pub type SteerImageResolver = Arc<dyn Fn(&[AttachmentId]) -> Vec<ModelImage> + Send + Sync>;

/// Synchronous recorder for the delivery outcomes of a steered correction's
/// attachments. Mirrors the seed-time rules (vision gate, budget, preview
/// fallback) so the durable item shows the same chips a turn start would.
pub type SteerOutcomeRecorder =
    Arc<dyn Fn(&[AttachmentId]) -> Vec<TurnAttachmentOutcome> + Send + Sync>;

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
    reasoning_effort: Option<String>,
    chunk_buffer: usize,
    machine: TurnMachine,
    history: Vec<ModelMessage>,
    pending_user_message: Option<String>,
    assistant_message_item: Option<StreamItem>,
    reasoning_item: Option<StreamItem>,
    /// Calls of the current round, in model order.
    pending_calls: Vec<ToolCallRequest>,
    /// Gates not yet resolved, in model order.
    pending_approvals: Vec<ApprovalAsk>,
    /// `request_user_input` calls of the current round, in model order. Each
    /// one is asked in its own round so every ask gets its own answer row
    /// (A2).
    pending_user_inputs: Vec<UserInputAsk>,
    /// Gate-free calls of the current round. They are parked while the turn
    /// is blocked on a sibling gate and dispatch together with the chain
    /// resolution: dropping them would orphan their ids in the assistant row
    /// (A1).
    plain_calls: Vec<ToolCallRequest>,
    /// Calls already approved during the current gate chain.
    approved: Vec<ToolCallRequest>,
    /// Synthetic results produced by earlier denials in the chain.
    denied: Vec<SyntheticResult>,
    /// Mid-turn corrections. Drained at a *round boundary* (before the next
    /// model call), never in the middle of a tool-call/result pair: injecting
    /// there would hand the provider a history it considers malformed.
    steer: Option<Arc<SteerQueue>>,
    /// Corrections drained for the in-flight round but not yet committed to
    /// history/journal. They join the request through `request()` and are only
    /// committed once the model round actually consumes them (AG-02). Each
    /// entry keeps the attachment ids it was queued with so a cancel restores
    /// it intact.
    staged_steering: Vec<StagedSteering>,
    /// Flat view of the ids above, in order, used by the image resolver at
    /// request-build time.
    staged_steering_attachments: Vec<AttachmentId>,
    /// Composition-root resolver turning staged ids into image parts.
    steer_image_resolver: Option<SteerImageResolver>,
    /// Composition-root recorder for the committed item's delivery outcomes.
    steer_outcome_recorder: Option<SteerOutcomeRecorder>,
    /// Guards against a second `turn_started` when `run()` is retried (AG-01).
    turn_started_written: bool,
    /// Recorded outcome, so a repeated `run()` is idempotent (AG-01).
    terminal_outcome: Option<TurnOutcome>,
    /// Monotonic gate id component so duplicate `call_id`s stay distinct.
    gate_seq: u64,
}

/// One correction staged for the in-flight model round: the text plus the
/// attachment ids it was queued with. Images resolve at request build time;
/// the ids ride along so a cancel can restore the entry intact.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StagedSteering {
    text: String,
    attachment_ids: Vec<AttachmentId>,
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
            steer_image_resolver,
            steer_outcome_recorder,
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
            reasoning_effort: seed.reasoning_effort,
            max_tokens: seed.max_tokens,
            chunk_buffer,
            machine,
            history: seed.messages,
            pending_user_message: None,
            assistant_message_item: None,
            reasoning_item: None,
            pending_calls: Vec::new(),
            pending_approvals: Vec::new(),
            pending_user_inputs: Vec::new(),
            plain_calls: Vec::new(),
            approved: Vec::new(),
            denied: Vec::new(),
            steer: None,
            staged_steering: Vec::new(),
            staged_steering_attachments: Vec::new(),
            steer_image_resolver,
            steer_outcome_recorder,
            turn_started_written: false,
            terminal_outcome: None,
            gate_seq: 0,
        }
    }

    /// Attach a steering queue so a running turn can be corrected mid-flight.
    pub fn with_steering(mut self, queue: Arc<SteerQueue>) -> Self {
        self.steer = Some(queue);
        self
    }

    /// Drain queued corrections into staged entries. They are *staged*, not
    /// folded into history: the journal may only claim a correction the model
    /// actually received.
    pub fn drain_steering(&mut self) -> Vec<String> {
        self.stage_steering();
        self.staged_steering
            .iter()
            .map(|entry| entry.text.clone())
            .collect()
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
        let text = text.into();
        self.pending_user_message = Some(text.clone());
        self.history.push(ModelMessage::user(text));
    }

    /// Append a fully built user message, e.g. one carrying multimodal image
    /// parts from the composition root. The pending-text bookkeeping stays on
    /// `content` so the existing round-start path is unchanged.
    pub fn push_user_message(&mut self, message: ModelMessage) {
        self.pending_user_message = Some(message.content.clone());
        self.history.push(message);
    }

    /// Write the user-visible recovery note for an automatic recovery turn.
    ///
    /// The previous turn died on a transient model-request failure; without
    /// this row the recovery looks like the model answering out of nowhere.
    /// The note is a `Notice` item: it renders in the transcript but never
    /// enters the model seed (`conversation_history` replays only user /
    /// assistant / checkpoint rows), so recovery bookkeeping cannot pollute
    /// the conversation context. Best effort: a broken writer must not cost
    /// the recovery its terminal event, mirroring `finish`.
    pub async fn note_auto_recovery(&mut self, attempt: u32, reason: &str) {
        let item = ThreadItem::new(self.events.turn_id, ItemKind::Notice)
            .with_text(format!(
                "[auto recovery #{attempt}] the previous turn failed on a transient model error \
             ({reason}); retrying automatically"
            ))
            .completed();
        let _ = self.writer.append(self.events.item_created(item)).await;
    }

    /// Run the turn. Every exit path records exactly one terminal event: an
    /// I/O failure inside the loop is converged into a `Failed` outcome here
    /// instead of leaving a `turn_started` with no terminal (AG-01).
    pub async fn run(&mut self) -> Result<TurnOutcome> {
        if self.machine.state().is_terminal() {
            return Ok(self
                .terminal_outcome
                .clone()
                .unwrap_or(TurnOutcome::Completed));
        }
        match self.drive().await {
            Ok(outcome) => Ok(outcome),
            Err(err) => {
                if !self.turn_started_written || self.machine.state().is_terminal() {
                    return Err(err);
                }
                let outcome = TurnOutcome::failed(err.message.clone());
                if self.machine.state() != TurnState::Finalizing {
                    let _ = self.machine.enter(&Action::Finish {
                        outcome: outcome.clone(),
                    });
                }
                match self.finish(outcome).await {
                    Ok(outcome) => Ok(outcome),
                    // A broken writer cannot record a terminal; keep the more
                    // diagnostic original error rather than masking it.
                    Err(_) => Err(err),
                }
            }
        }
    }

    /// The loop body. Errors bubble to `run`, which owns terminalization.
    async fn drive(&mut self) -> Result<TurnOutcome> {
        self.prepare().await?;
        let mut input = TurnInput::new(
            self.machine.limits(),
            self.machine.progress(),
            InputKind::Start,
        );
        loop {
            let action = step(self.machine.state(), &input);
            let action = match action {
                Action::Finish { outcome } => self.rescue_pending_steering(outcome).await?,
                other => other,
            };
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
        if !self.turn_started_written {
            self.writer.append(self.events.turn_started()).await?;
            self.turn_started_written = true;
        }
        if let Some(text) = self.pending_user_message.take() {
            // The turn's opening user item carries the attachment outcomes so
            // the durable item renders attachment chips after a replay, not
            // only while the live `turn_started` is observable. Steering
            // messages have no attachments of their own and stay bare.
            let mut item = ThreadItem::new(self.events.turn_id, ItemKind::UserMessage)
                .with_text(text)
                .completed();
            if !self.events.attachment_outcomes().is_empty() {
                item = item.with_attachments(self.events.attachment_outcomes().to_vec());
            }
            self.writer.append(self.events.item_created(item)).await?;
        }
        let report = HistoryHygiene::new().repair(&mut self.history);
        debug_assert!(
            HistoryHygiene::new().is_legal(&self.history),
            "history must be provider-legal after repair, report: {report:?}"
        );
        // `Start` is budget-checked before the first model round. Include the
        // just-persisted user message here, otherwise an oversized initial
        // prompt bypasses compaction until after it has already reached the
        // provider.
        let estimate = TokenEstimator::new().estimate_request(&self.request());
        self.machine.set_prompt_estimate(estimate);
        Ok(())
    }

    /// A cancel that arrives between actions terminates the turn immediately.
    fn cancelled_outcome(&self) -> Option<TurnOutcome> {
        if self.cancel.is_cancelled() && !self.machine.state().is_terminal() {
            Some(TurnOutcome::aborted(
                self.cancellation_reason("turn interrupted"),
            ))
        } else {
            None
        }
    }

    /// Preserve a boundary-supplied detail for ordinary interrupts, while a
    /// process-owned reason such as shutdown remains distinguishable on disk.
    /// Only the shared `UserInterrupt` vocabulary maps to the fallback; an
    /// unknown string passes through unchanged (the previous behavior), so a
    /// future durable reason is never silently overwritten here.
    fn cancellation_reason(&self, fallback: &str) -> String {
        match self.cancel.reason().as_deref() {
            Some(reason) => match ark_common::CancelReason::from_durable(reason) {
                Some(ark_common::CancelReason::UserInterrupt) => fallback.to_string(),
                _ => reason.to_string(),
            },
            None => fallback.to_string(),
        }
    }

    /// Resolves the staged corrections' attachment ids into image parts. With
    /// no resolver installed (tests, non-attachment runtimes) a steer stays
    /// text-only.
    fn staged_images(&self) -> Vec<ModelImage> {
        match &self.steer_image_resolver {
            Some(resolve) if !self.staged_steering_attachments.is_empty() => {
                resolve(&self.staged_steering_attachments)
            }
            _ => Vec::new(),
        }
    }

    fn request(&self) -> ModelRequest {
        let mut messages = self.history.clone();
        // Staged corrections travel with the request but are committed to
        // `history` only once the stream confirms the model consumed them.
        // Attachments referenced by a steer resolve through the composition
        // root's resolver (text keeps the preview fallback path); the reactor
        // never reads the filesystem itself.
        for staged in &self.staged_steering {
            let mut message = ModelMessage::user(staged.text.clone());
            message.images = self.staged_images();
            messages.push(message);
        }
        ModelRequest {
            messages,
            tools: self.tools_schema.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            reasoning_effort: self.reasoning_effort.clone(),
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
    ///
    /// Corrections are *staged* into the request. They are committed to history
    /// and the journal only after the model round was actually sent; a cancel
    /// inside `pump` restores them to the queue instead of silently dropping a
    /// 202-accepted steer (AG-02).
    async fn round_input(&mut self) -> Result<TurnInput> {
        self.stage_steering();
        let estimate = TokenEstimator::new().estimate_request(&self.request());
        self.machine.set_prompt_estimate(estimate);
        match self.pump().await? {
            Round::Settled(response) => {
                self.commit_steering().await?;
                self.settle_model(response).await
            }
            Round::Failed(message) => {
                self.commit_steering().await?;
                Ok(self.input_of(InputKind::ModelFailed { message }))
            }
            Round::Aborted => {
                self.restore_steering();
                let reason = self.cancellation_reason("turn interrupted");
                Ok(self.interrupt_input(&reason))
            }
        }
    }

    fn stage_steering(&mut self) {
        let entries = self.steer.as_ref().map(|q| q.drain()).unwrap_or_default();
        self.staged_steering = entries
            .iter()
            .map(|entry| StagedSteering {
                text: entry.text.clone(),
                attachment_ids: entry.attachment_ids.clone(),
            })
            .collect();
        self.staged_steering_attachments = entries
            .iter()
            .flat_map(|entry| entry.attachment_ids.iter().copied())
            .collect();
    }

    /// Commit staged corrections to history + durable journal. Only called
    /// after the model request really carried them. Each correction becomes a
    /// `UserMessage` item; the attachments it referenced record their delivery
    /// outcome on the item (image/preview/degraded/…), so replay renders the
    /// same chips the live stream showed. A queue without ids produces the
    /// old bare item.
    async fn commit_steering(&mut self) -> Result<()> {
        self.staged_steering_attachments.clear();
        for staged in std::mem::take(&mut self.staged_steering) {
            self.history.push(ModelMessage::user(staged.text.clone()));
            self.writer
                .append(self.events.turn_steered(&staged.text))
                .await?;
            let mut item = ThreadItem::new(self.events.turn_id, ItemKind::UserMessage)
                .with_text(staged.text)
                .completed();
            if let Some(resolve) = &self.steer_outcome_recorder {
                let outcomes = resolve(&staged.attachment_ids);
                if !outcomes.is_empty() {
                    item = item.with_attachments(outcomes);
                }
            }
            self.writer.append(self.events.item_created(item)).await?;
        }
        Ok(())
    }

    /// Put staged corrections back in order so a retry can deliver them, with
    /// the attachment ids they were queued with (AG-02).
    fn restore_steering(&mut self) {
        let staged = std::mem::take(&mut self.staged_steering);
        self.staged_steering_attachments.clear();
        if let Some(queue) = &self.steer {
            queue.restore(
                staged
                    .into_iter()
                    .map(|entry| SteerEntry {
                        text: entry.text,
                        attachment_ids: entry.attachment_ids,
                    })
                    .collect(),
            );
        }
    }

    /// Stage + commit for the rescue path (no model stream of its own).
    async fn apply_steering(&mut self) -> Result<()> {
        self.stage_steering();
        self.commit_steering().await
    }

    fn has_pending_steering(&self) -> bool {
        self.steer.as_ref().is_some_and(|queue| !queue.is_empty())
    }

    /// A correction that lands during the *final* model round would otherwise be
    /// lost: the queue only drains at round starts, and a tool-less settle ends
    /// the turn without another round — while the server has already answered
    /// 202 to the steer. Convert the finish into a zero-batch dispatch so the
    /// correction is journaled and the next round consumes it, still bounded by
    /// the round budget. A cancelled or budget-exhausted turn keeps its original
    /// finish instead: journaling the correction there would claim a message
    /// the model never sees, because the rescue round would abort at the next
    /// cancel check or fail the budget check after `turn_steered` was written.
    async fn rescue_pending_steering(&mut self, outcome: TurnOutcome) -> Result<Action> {
        if outcome != TurnOutcome::Completed
            || !self.has_pending_steering()
            || self.cancel.is_cancelled()
            || !matches!(
                TurnBudget::new(self.machine.limits()).check(&self.machine.progress()),
                BudgetVerdict::Allow
            )
        {
            return Ok(Action::Finish { outcome });
        }
        self.apply_steering().await?;
        Ok(Action::Dispatch(DispatchPlan::default()))
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
                let item = self.ensure_stream_item(ItemKind::AssistantMessage).await?;
                self.writer
                    .append(self.events.text_delta(item, delta.clone()))
                    .await?;
                self.stream_item_mut(ItemKind::AssistantMessage)?
                    .text
                    .push_str(&delta);
                acc.text.push_str(&delta);
            }
            ModelChunk::ReasoningDelta { delta } => {
                let item = self.ensure_stream_item(ItemKind::Reasoning).await?;
                self.writer
                    .append(self.events.reasoning_delta(item, delta.clone()))
                    .await?;
                self.stream_item_mut(ItemKind::Reasoning)?
                    .text
                    .push_str(&delta);
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

    async fn ensure_stream_item(&mut self, kind: ItemKind) -> Result<ItemId> {
        let missing = match kind {
            ItemKind::AssistantMessage => self.assistant_message_item.is_none(),
            ItemKind::Reasoning => self.reasoning_item.is_none(),
            _ => unreachable!("only streamed item kinds are requested"),
        };
        if missing {
            let item = ThreadItem::new(self.events.turn_id, kind);
            self.writer
                .append(self.events.item_created(item.clone()))
                .await?;
            match kind {
                ItemKind::AssistantMessage => {
                    self.assistant_message_item = Some(StreamItem::from(item))
                }
                ItemKind::Reasoning => self.reasoning_item = Some(StreamItem::from(item)),
                _ => unreachable!("only streamed item kinds are requested"),
            }
        }
        Ok(self.stream_item_mut(kind)?.item_id)
    }

    fn stream_item_mut(&mut self, kind: ItemKind) -> Result<&mut StreamItem> {
        let item = match kind {
            ItemKind::AssistantMessage => self.assistant_message_item.as_mut(),
            ItemKind::Reasoning => self.reasoning_item.as_mut(),
            _ => unreachable!("only streamed item kinds are requested"),
        };
        match item {
            Some(item) => Ok(item),
            None => Err(ArkError::internal("stream item was not created")),
        }
    }

    /// Complete each item opened by the current model round exactly once.
    async fn complete_stream_items(&mut self) -> Result<()> {
        for item in [
            self.assistant_message_item.take(),
            self.reasoning_item.take(),
        ]
        .into_iter()
        .flatten()
        {
            let completed = ThreadItem {
                item_id: item.item_id,
                turn_id: self.events.turn_id,
                kind: item.kind,
                status: ark_contract::ItemStatus::Completed,
                created_at: chrono::Utc::now(),
                text: Some(item.text),
                tool: None,
                detail: None,
                // StreamItem only buffers assistant/reasoning text, which never
                // carries a marker or attachments.
                marker: None,
                attachments: None,
            };
            self.writer
                .append(self.events.item_completed(completed))
                .await?;
        }
        Ok(())
    }

    /// Fold the settled response into history and build the `ModelSettled` input.
    async fn settle_model(&mut self, response: ModelResponse) -> Result<TurnInput> {
        let usage = response.usage.clone();
        let mut calls = Vec::new();
        let mut user_inputs = Vec::new();
        for (call_id, tool, arguments) in response.tool_calls.clone() {
            // The policy is consulted **once**, here, and the decision travels
            // with the call. The executor consumes it instead of asking again,
            // which is what makes an approved `bash` actually runnable
            // (design §7.3, P1-5).
            let decision = self.sandbox.decide(&tool, &ToolArgs(arguments.clone()));
            let call = ToolCallRequest::new(call_id, tool, arguments).with_sandbox(decision);
            // The user-input tool is intercepted before the dispatch plan is
            // built: it answers by asking a human, never by executing.
            if call.tool == USER_INPUT_TOOL {
                let prompt = extract_user_input_prompt(&call.arguments);
                user_inputs.push(UserInputAsk::new(call.clone(), prompt));
            }
            calls.push(call);
        }
        self.record_assistant_turn(&response.text, &calls);
        self.machine.note_usage(&usage);
        self.writer.append(self.events.usage(usage.clone())).await?;
        self.complete_stream_items().await?;

        self.pending_calls = calls.clone();
        self.pending_user_inputs = user_inputs.clone();
        self.approved.clear();
        self.denied.clear();
        self.pending_approvals = self.approval_asks(&calls);
        // Everything that is neither an approval ask nor a user-input ask
        // runs without a gate; it must survive the gate chain untouched,
        // otherwise its ids stay orphaned in the assistant row (A1).
        let gated: std::collections::HashSet<&str> = self
            .pending_approvals
            .iter()
            .map(|ask| ask.call.call_id.as_str())
            .chain(
                self.pending_user_inputs
                    .iter()
                    .map(|ask| ask.call.call_id.as_str()),
            )
            .collect();
        self.plain_calls = calls
            .iter()
            .filter(|call| !gated.contains(call.call_id.as_str()))
            .cloned()
            .collect();
        let repeats = self.machine.repeats_for(&calls);
        let approvals = self.pending_approvals.clone();
        Ok(self
            .input_of(InputKind::ModelSettled {
                text: response.text,
                tool_calls: calls,
                repeats,
                usage,
            })
            .with_approvals(approvals)
            .with_user_inputs(user_inputs))
    }

    /// Record what the model said. Tool calls are attached to the assistant
    /// row via `tool_calls` so the following `ToolRole` rows stay paired;
    /// providers reject orphan tool messages.
    fn record_assistant_turn(&mut self, text: &str, calls: &[ToolCallRequest]) {
        if calls.is_empty() {
            if !text.is_empty() {
                self.history.push(ModelMessage::assistant(text));
            }
            return;
        }
        let calls: Vec<ToolCallRef> = calls
            .iter()
            .map(|call| ToolCallRef {
                call_id: call.call_id.clone(),
                name: call.tool.clone(),
                arguments: call.arguments.clone(),
            })
            .collect();
        self.history
            .push(ModelMessage::assistant_with_tool_calls(text, calls));
    }

    /// Write the terminal event, then land on the terminal state.
    async fn finish(&mut self, outcome: TurnOutcome) -> Result<TurnOutcome> {
        // Rendering items are best-effort: their append must never cost the
        // turn its terminal event (the old ordering did exactly that, AG-01).
        let _ = self.complete_stream_items().await;
        self.writer.finish(&outcome).await?;
        if self.machine.state() == TurnState::Finalizing {
            self.machine
                .mark_terminal()
                .map_err(|err| ArkError::internal(err.to_string()))?;
        }
        self.machine
            .settle(&outcome)
            .map_err(|err| ArkError::internal(err.to_string()))?;
        self.terminal_outcome = Some(outcome.clone());
        Ok(outcome)
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
            let detail = "cancelled before the call completed".to_string();
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
                detail: Some(detail),
            };
            self.append_tool_history(call, &execution);
        }
        Ok(())
    }
}

/// A streamed render item kept until its model round reaches a terminal edge.
struct StreamItem {
    item_id: ItemId,
    kind: ItemKind,
    text: String,
}

impl From<ThreadItem> for StreamItem {
    fn from(item: ThreadItem) -> Self {
        Self {
            item_id: item.item_id,
            kind: item.kind,
            text: item.text.unwrap_or_default(),
        }
    }
}

/// Render a call for the approval prompt.
fn summarize(call: &ToolCallRequest) -> String {
    let arguments = call.arguments.to_string();
    let args = ark_common::text::truncate_utf8(&arguments, 240);
    format!("{} {args}", call.tool)
}

/// The question shown to the user. Contract: `{"prompt": string}`; `question`
/// is accepted as a legacy alias so a model that guessed the field name still
/// reaches the gate instead of surfacing an empty prompt.
fn extract_user_input_prompt(arguments: &serde_json::Value) -> String {
    ["prompt", "question"]
        .iter()
        .find_map(|key| arguments.get(key).and_then(|value| value.as_str()))
        .unwrap_or_default()
        .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    // The inline suite lives in its own file (AGENTS §4 file-size limit);
    // declared as a descendant so `use super::*` reaches the shared doubles
    // and the private reactor methods, exactly as the gate/regression files.
    #[path = "reactor_stream_tests.rs"]
    mod stream_tests;
}
