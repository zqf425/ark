//! Human gates: which calls need a decision, and how an approval or a
//! user-input answer becomes the next turn input.

use super::*;

impl Reactor {
    /// Calls that need a human decision.
    ///
    /// Reads the decision already attached by [`Self::settle_model`]. The policy
    /// fallback exists only for calls constructed outside that path (synthetic
    /// or test calls) and keeps the old behaviour for them.
    pub(super) fn approval_asks(&self, calls: &[ToolCallRequest]) -> Vec<ApprovalAsk> {
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
    pub(super) async fn ask_approval(
        &mut self,
        mut call: ToolCallRequest,
        summary: String,
    ) -> Result<TurnInput> {
        // `call_id` is provider-local and may repeat even within one round;
        // HTTP resolution is process-wide, so scope the public gate id by turn
        // and a monotonic sequence (AG-05). The id still ends with the call id
        // so gate-ordering assertions stay stable.
        self.gate_seq = self.gate_seq.saturating_add(1);
        let approval_id = format!(
            "approval_{}_{}_{}",
            self.events.turn_id, self.gate_seq, call.call_id
        );
        let gates = Arc::clone(&self.gates);
        let request = ApprovalRequest::new(approval_id.clone(), call.clone(), summary.clone());
        // Prepare **before** the event becomes observable: prepare completes
        // the registration synchronously, so the moment a client sees
        // `approval_requested` it may resolve over HTTP. Writing the event
        // first left a real 404 window (observed 4/4 against a live harness).
        let reply_future = gates.prepare_approval(request)?;
        self.writer
            .append(
                self.events
                    .approval_requested(&approval_id, &call.tool, &summary),
            )
            .await?;
        let reply = tokio::select! {
            _ = self.cancel.cancelled() => {
                // The assistant row already declares every call of this round.
                // Close the ones still unanswered before unwinding, or the
                // journal and history are left with orphan tool_calls (AG-03).
                self.converge_pending_calls_on_cancel().await?;
                let reason = self.cancellation_reason("turn interrupted while awaiting approval");
                return Ok(self.interrupt_input(&reason))
            }
            reply = reply_future => reply?,
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
        // Remove exactly one entry: `retain` deleted every ask sharing the
        // same provider-local `call_id`, silently dropping sibling calls
        // (AG-05).
        if let Some(pos) = self
            .pending_approvals
            .iter()
            .position(|ask| ask.call.call_id == call.call_id)
        {
            self.pending_approvals.remove(pos);
        }
        if reply.approved() {
            // A human approval is scoped to this exact, already-validated call.
            // The runner rejects `NeedsApproval` defensively, so it must receive
            // a concrete executable decision rather than re-consulting policy.
            call.sandbox = Some(SandboxDecision::WorkspaceWrite);
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
            // The gate chain is done: the round's gate-free calls ride along
            // with the resolution. Dropping them would leave their
            // `tool_call` entries in the assistant row without a result row,
            // which providers reject (A1).
            let mut pending = self.plain_calls.clone();
            pending.extend(already_approved);
            pending
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
    pub(super) async fn ask_user(
        &mut self,
        call: ToolCallRequest,
        prompt: String,
    ) -> Result<TurnInput> {
        self.gate_seq = self.gate_seq.saturating_add(1);
        let request_id = format!(
            "ask_{}_{}_{}",
            self.events.turn_id, self.gate_seq, call.call_id
        );
        let gates = Arc::clone(&self.gates);
        let request = UserInputRequest {
            request_id: request_id.clone(),
            call: call.clone(),
            prompt: prompt.clone(),
        };
        // Same ordering as approvals: prepare registers synchronously before
        // the event is observable, so an immediate HTTP answer can never miss
        // the window.
        let answer_future = gates.prepare_user_input(request)?;
        self.writer
            .append(self.events.user_input_requested(&request_id, &prompt))
            .await?;
        let answer = tokio::select! {
            _ = self.cancel.cancelled() => {
                // Same pairing invariant as approvals (AG-03).
                self.converge_pending_calls_on_cancel().await?;
                let reason = self.cancellation_reason("turn interrupted while awaiting user input");
                return Ok(self.interrupt_input(&reason))
            }
            answer = answer_future => answer?,
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
                answer.clone(),
                None,
            ))
            .await?;
        // Chain bookkeeping for the sibling gates of the same round: every ask
        // is answered in its own round, approvals follow once the asks are
        // done, and only then may anything dispatch (A2).
        if let Some(pos) = self
            .pending_user_inputs
            .iter()
            .position(|ask| ask.call.call_id == call.call_id)
        {
            self.pending_user_inputs.remove(pos);
        }
        let remaining_asks = self.pending_user_inputs.clone();
        let approvals = self.pending_approvals.clone();
        let pending_dispatch = if remaining_asks.is_empty() && approvals.is_empty() {
            self.plain_calls.clone()
        } else {
            Vec::new()
        };
        let repeats = self.machine.repeats_for(&pending_dispatch);
        Ok(self.input_of(InputKind::UserInputResolved {
            call: call.clone(),
            answer,
            remaining_asks,
            approvals,
            pending_dispatch,
            repeats,
        }))
    }
}
