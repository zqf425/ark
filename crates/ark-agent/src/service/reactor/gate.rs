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
        // `call_id` is provider-local and may repeat across concurrent turns;
        // HTTP resolution is process-wide, so scope the public gate id by turn.
        let approval_id = format!("approval_{}_{}", self.events.turn_id, call.call_id);
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
                // No explicit drop: the registered future was moved into this
                // select branch, and `select!` drops the losing branch's
                // future, whose internal guard removes the registration.
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
        self.pending_approvals
            .retain(|ask| ask.call.call_id != call.call_id);
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
    pub(super) async fn ask_user(
        &mut self,
        call: ToolCallRequest,
        prompt: String,
    ) -> Result<TurnInput> {
        let request_id = format!("ask_{}_{}", self.events.turn_id, call.call_id);
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
                // Same as approvals: the losing select branch's future is
                // dropped and its internal guard cleans up the registration.
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
        Ok(self.input_of(InputKind::UserInputResolved {
            call: call.clone(),
            answer,
        }))
    }
}
