//! Builtin `delegate_task` tool: hand one bounded sub-task to a child run
//! (design §7.8 / §7.14).
//!
//! The tool deliberately knows almost nothing. It does **not** create sessions,
//! enforce the concurrency budget, or decide whether delegation is enabled: it
//! asks [`SubagentRunner`], which is the runtime that owns the policy, the run
//! log and the abort/steer handles. That keeps `ark-tool` free of any dependency
//! on `ark-delegation`, the store, or the model wiring.
//!
//! Two things this tool must never do:
//!
//! * hand back the child's raw event log — the parent model reads the child's
//!   final answer, so `content` is that answer and nothing else;
//! * let a child delegate again. `arkd` builds child tool sets without this
//!   tool, which is what enforces `max_depth = 1`.

use std::sync::Arc;

use ark_ports::{
    SubagentOutcome, SubagentRequest, SubagentRunner, Tool, ToolArgs, ToolContext, ToolOutput,
};

use crate::domain::{args, schema};
use crate::exec::common::as_output;

/// `delegate_task`.
///
/// Held behind an `Arc<dyn SubagentRunner>` rather than a concrete runtime, so a
/// test can pass a fake and `arkd` can pass the real one.
pub struct DelegateTaskTool {
    runner: Arc<dyn SubagentRunner>,
}

impl DelegateTaskTool {
    pub fn new(runner: Arc<dyn SubagentRunner>) -> Self {
        Self { runner }
    }

    pub fn runner(&self) -> &Arc<dyn SubagentRunner> {
        &self.runner
    }
}

impl std::fmt::Debug for DelegateTaskTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegateTaskTool")
            .field("enabled", &self.runner.is_enabled())
            .field("active_children", &self.runner.active_count())
            .finish()
    }
}

#[async_trait::async_trait]
impl Tool for DelegateTaskTool {
    fn name(&self) -> &'static str {
        "delegate_task"
    }

    fn schema(&self) -> &'static serde_json::Value {
        schema::delegate_task()
    }

    async fn execute(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        let parsed: args::DelegateArgs = match args::parse("delegate_task", &raw) {
            Ok(parsed) => parsed,
            // A malformed call is reported as content, not as a failed dispatch:
            // the model can fix its arguments and try again.
            Err(err) => return Ok(ToolOutput::err(err.message)),
        };
        Ok(as_output(self.run(ctx, parsed).await))
    }
}

impl DelegateTaskTool {
    async fn run(
        &self,
        ctx: &ToolContext,
        parsed: args::DelegateArgs,
    ) -> ark_common::Result<ToolOutput> {
        // Checked here as well as inside the runtime so a disabled feature
        // returns an actionable message instead of a `Conflict` the model is
        // likely to retry verbatim.
        if !self.runner.is_enabled() {
            return Ok(ToolOutput::err(
                "sub-agents are disabled; enable them in Settings to delegate",
            ));
        }

        let background = parsed.background;
        let resume_child_id = parsed.resume_child_id;
        let request = SubagentRequest {
            // Identity always comes from the caller's context, never from the
            // model: a child can only ever attach to the thread that spawned it.
            parent_thread_id: ctx.thread_id,
            parent_turn_id: ctx.turn_id,
            label: parsed.label.unwrap_or_default(),
            prompt: parsed.prompt,
            // Workspace is **inherited**, never supplied by the model. A child
            // that could name its own root would effectively widen the parent's
            // sandbox (design §7.3): every write tool judges containment against
            // this value.
            workspace: Some(ctx.workspace.display().to_string()),
            model: parsed.model,
            provider_id: parsed.provider_id,
            resume_child_id,
        };

        // Resume keeps the foreground result contract even if an old caller also
        // sends `background: true`; this matches the original kun precedence and
        // avoids acknowledging a correction without returning its result.
        if resume_child_id.is_some() || !background {
            let outcome = self.runner.delegate(request).await?;
            return Ok(render(&outcome));
        }

        let child = self.runner.delegate_background(request).await?;
        Ok(ToolOutput {
            ok: true,
            content: format!("child run {child} queued in the background"),
            detail: Some(format!("child={child} status=queued")),
        })
    }
}

/// Turn an outcome into what the parent model reads.
///
/// `content` is the child's final answer; everything the UI needs (child id,
/// status, tool count, real usage counters) travels in `detail`. Usage is copied
/// verbatim from the child's own accounting — never recomputed or estimated.
fn render(outcome: &SubagentOutcome) -> ToolOutput {
    let body = match (&outcome.summary, &outcome.error) {
        (Some(summary), _) if !summary.trim().is_empty() => summary.clone(),
        (_, Some(error)) if !error.trim().is_empty() => {
            format!("the child run stopped before answering: {error}")
        }
        _ => "(the child returned no summary)".to_string(),
    };

    let mut detail = vec![
        format!("child={}", outcome.child_id),
        format!("status={}", outcome.status),
        format!("tools={}", outcome.tool_invocations),
    ];
    if let Some(model) = &outcome.model {
        detail.push(format!("model={model}"));
    }
    if let Some(provider) = &outcome.provider_id {
        detail.push(format!("provider={provider}"));
    }
    let usage = &outcome.usage;
    if usage.total_tokens > 0 {
        detail.push(format!(
            "tokens={}/{}/{}",
            usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
        ));
    }

    ToolOutput {
        // An aborted child still produced something the parent can use, which is
        // exactly what `is_usable` encodes.
        ok: outcome.is_usable(),
        content: body,
        detail: Some(detail.join(" ")),
    }
}

#[cfg(test)]
mod tests;
