//! `delegate_task` tests.
//!
//! The runner is a fake on purpose. This crate must not know how a child run is
//! executed, so what is worth testing here is the contract the parent model sees:
//! which request travels out, what the answer looks like, and that a refusal is
//! reported honestly instead of being dressed up as a result.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use ark_common::{ArkError, ArkErrorCode, ChildId, ThreadId, TurnId, Usage};
use ark_ports::{
    SandboxDecision, SubagentOutcome, SubagentRequest, SubagentRunner, Tool, ToolArgs, ToolContext,
};

use super::*;

/// Runner that records requests and replies with a fixed outcome.
struct FakeRunner {
    enabled: bool,
    reply: Result<SubagentOutcome, ArkErrorCode>,
    seen: Mutex<Vec<SubagentRequest>>,
    active: AtomicU32,
    foreground_calls: AtomicU32,
    background_calls: AtomicU32,
}

impl FakeRunner {
    fn new(enabled: bool, reply: SubagentOutcome) -> Arc<Self> {
        Arc::new(Self {
            enabled,
            // Keep the caller's `child_id`: the test asserts the parent sees the
            // real child identity, so the fake must not mint its own.
            reply: Ok(reply),
            seen: Mutex::new(Vec::new()),
            active: AtomicU32::new(0),
            foreground_calls: AtomicU32::new(0),
            background_calls: AtomicU32::new(0),
        })
    }

    fn failing(enabled: bool, code: ArkErrorCode) -> Arc<Self> {
        Arc::new(Self {
            enabled,
            reply: Err(code),
            seen: Mutex::new(Vec::new()),
            active: AtomicU32::new(0),
            foreground_calls: AtomicU32::new(0),
            background_calls: AtomicU32::new(0),
        })
    }

    fn requests(&self) -> Vec<SubagentRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn background_calls(&self) -> u32 {
        self.background_calls.load(Ordering::Relaxed)
    }

    fn foreground_calls(&self) -> u32 {
        self.foreground_calls.load(Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl SubagentRunner for FakeRunner {
    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn active_count(&self) -> u32 {
        self.active.load(Ordering::Relaxed)
    }

    async fn delegate(&self, request: SubagentRequest) -> ark_common::Result<SubagentOutcome> {
        self.foreground_calls.fetch_add(1, Ordering::Relaxed);
        self.seen.lock().unwrap().push(request);
        match &self.reply {
            Ok(outcome) => Ok(outcome.clone()),
            Err(code) => Err(ArkError::new(*code, "runner refused")),
        }
    }

    async fn delegate_background(&self, request: SubagentRequest) -> ark_common::Result<ChildId> {
        self.background_calls.fetch_add(1, Ordering::Relaxed);
        self.seen.lock().unwrap().push(request);
        match &self.reply {
            Ok(outcome) => Ok(outcome.child_id),
            Err(code) => Err(ArkError::new(*code, "runner refused")),
        }
    }

    /// `delegate_task` does not use the prepare/run split; these methods only
    /// satisfy the runner port for this focused tool test.
    async fn prepare_child(&self, request: SubagentRequest) -> ark_common::Result<ChildId> {
        let child = match &self.reply {
            Ok(outcome) => outcome.child_id,
            Err(code) => return Err(ArkError::new(*code, "runner refused")),
        };
        self.seen.lock().unwrap().push(request);
        Ok(child)
    }

    async fn run_child(&self, child: ChildId) -> ark_common::Result<SubagentOutcome> {
        match &self.reply {
            Ok(outcome) => Ok(SubagentOutcome {
                child_id: child,
                ..outcome.clone()
            }),
            Err(code) => Err(ArkError::new(*code, "runner refused")),
        }
    }
}

fn ctx() -> ToolContext {
    ToolContext {
        thread_id: ThreadId::new(),
        turn_id: TurnId::new(),
        workspace: std::path::PathBuf::from("/tmp/workspace"),
        sandbox: SandboxDecision::ReadOnly,
        env: std::collections::BTreeMap::new(),
    }
}

fn completed(summary: &str) -> SubagentOutcome {
    SubagentOutcome {
        child_id: ChildId::new(),
        status: ark_ports::child_status::COMPLETED.to_string(),
        summary: Some(summary.to_string()),
        error: None,
        usage: Usage::default(),
        tool_invocations: 0,
        model: None,
        provider_id: None,
    }
}

#[tokio::test]
async fn a_prompt_alone_is_enough_to_spawn_a_child() {
    let runner = FakeRunner::new(true, completed("done"));
    let tool = DelegateTaskTool::new(runner.clone());
    let context = ctx();

    let out = tool
        .execute(
            &context,
            ToolArgs(serde_json::json!({ "prompt": "scan src" })),
        )
        .await
        .unwrap();

    assert!(out.ok);
    assert_eq!(out.content, "done");

    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, "scan src");
    // Identity always comes from the context, never from the model's arguments.
    assert_eq!(requests[0].parent_thread_id, context.thread_id);
    assert_eq!(requests[0].parent_turn_id, context.turn_id);
    // The workspace defaults to the parent's, so a child cannot silently escape.
    assert_eq!(
        requests[0].workspace.as_deref(),
        Some(context.workspace.display().to_string().as_str())
    );
    assert!(requests[0].resume_child_id.is_none());
    assert_eq!(runner.foreground_calls(), 1);
    assert_eq!(runner.background_calls(), 0);
}

#[tokio::test]
async fn background_returns_the_queued_child_without_using_foreground_delegate() {
    let outcome = completed("unused foreground answer");
    let child = outcome.child_id;
    let runner = FakeRunner::new(true, outcome);
    let tool = DelegateTaskTool::new(runner.clone());

    let out = tool
        .execute(
            &ctx(),
            ToolArgs(serde_json::json!({ "prompt": "scan", "background": true })),
        )
        .await
        .unwrap();

    assert!(out.ok);
    assert!(out.content.contains(&child.to_string()), "{}", out.content);
    let expected_detail = format!("child={child} status=queued");
    assert_eq!(out.detail.as_deref(), Some(expected_detail.as_str()));
    assert_eq!(runner.background_calls(), 1);
    assert_eq!(runner.foreground_calls(), 0);
    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].resume_child_id.is_none());
}

#[tokio::test]
async fn resume_child_id_takes_precedence_over_background() {
    let resumed = ChildId::new();
    let runner = FakeRunner::new(true, completed("resumed answer"));
    let tool = DelegateTaskTool::new(runner.clone());

    let out = tool
        .execute(
            &ctx(),
            ToolArgs(serde_json::json!({
                "prompt": "continue with this correction",
                "background": true,
                "resumeChildId": resumed.to_string(),
            })),
        )
        .await
        .unwrap();

    assert!(out.ok);
    assert_eq!(out.content, "resumed answer");
    assert_eq!(runner.foreground_calls(), 1);
    assert_eq!(runner.background_calls(), 0);
    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].resume_child_id, Some(resumed));
}

#[tokio::test]
async fn a_child_cannot_choose_its_own_workspace() {
    // The child's workspace is inherited from the parent context. A model that
    // tries to name a directory must not be able to widen the sandbox, so the
    // unknown argument is ignored and the context value always wins.
    let runner = FakeRunner::new(true, completed("ok"));
    let tool = DelegateTaskTool::new(runner.clone());
    let context = ctx();

    tool.execute(
        &context,
        ToolArgs(serde_json::json!({
            "prompt": "inspect",
            "workspace": "C:/Windows",
        })),
    )
    .await
    .unwrap();

    let expected = context.workspace.display().to_string();
    assert_eq!(
        runner.requests()[0].workspace.as_deref(),
        Some(expected.as_str())
    );
}

#[tokio::test]
async fn the_model_can_override_the_label_model_and_workspace() {
    let runner = FakeRunner::new(true, completed("ok"));
    let tool = DelegateTaskTool::new(runner.clone());

    tool.execute(
        &ctx(),
        ToolArgs(serde_json::json!({
            "prompt": "inspect",
            "label": "reviewer",
            "model": "other-model",
            "provider_id": "other-provider",
            "workspace": "/tmp/elsewhere",
        })),
    )
    .await
    .unwrap();

    let request = runner.requests().remove(0);
    assert_eq!(request.label, "reviewer");
    assert_eq!(request.model.as_deref(), Some("other-model"));
    assert_eq!(request.provider_id.as_deref(), Some("other-provider"));
    // Workspace is always the parent's, even when the model supplies one.
    let expected = ctx().workspace.display().to_string();
    assert_eq!(request.workspace.as_deref(), Some(expected.as_str()));
}

#[tokio::test]
async fn a_disabled_runtime_reports_it_without_calling_the_runner() {
    let runner = FakeRunner::new(false, completed("unused"));
    let tool = DelegateTaskTool::new(runner.clone());

    let out = tool
        .execute(&ctx(), ToolArgs(serde_json::json!({ "prompt": "go" })))
        .await
        .unwrap();

    assert!(!out.ok);
    assert!(out.content.contains("disabled"), "{}", out.content);
    assert!(
        runner.requests().is_empty(),
        "a disabled runtime must not be asked at all"
    );
}

#[tokio::test]
async fn a_saturated_budget_surfaces_as_a_failed_call() {
    let runner = FakeRunner::failing(true, ArkErrorCode::Conflict);
    let tool = DelegateTaskTool::new(runner);

    // Every builtin tool reports a dispatch error as `ok: false` content (see
    // `exec::common::as_output`), so the model can read the reason and adapt
    // instead of the turn failing opaquely.
    let out = tool
        .execute(&ctx(), ToolArgs(serde_json::json!({ "prompt": "go" })))
        .await
        .unwrap();

    assert!(!out.ok, "a refused spawn is not a successful call");
    assert!(out.content.contains("runner refused"), "{}", out.content);
}

#[tokio::test]
async fn a_missing_prompt_is_reported_as_content_so_the_model_can_retry() {
    let runner = FakeRunner::new(true, completed("unused"));
    let tool = DelegateTaskTool::new(runner.clone());

    let out = tool
        .execute(&ctx(), ToolArgs(serde_json::json!({ "label": "x" })))
        .await
        .unwrap();

    assert!(!out.ok);
    assert!(out.content.contains("delegate_task"), "{}", out.content);
    assert!(runner.requests().is_empty());
}

#[tokio::test]
async fn an_empty_summary_is_not_silently_rendered_as_success_content() {
    let runner = FakeRunner::new(true, completed(""));
    let tool = DelegateTaskTool::new(runner);

    let out = tool
        .execute(&ctx(), ToolArgs(serde_json::json!({ "prompt": "go" })))
        .await
        .unwrap();

    assert_eq!(out.content, "(the child returned no summary)");
}

#[tokio::test]
async fn a_failed_child_reports_its_error_text_to_the_parent() {
    let mut outcome = completed("");
    outcome.status = ark_ports::child_status::FAILED.to_string();
    outcome.summary = None;
    outcome.error = Some("model refused".to_string());
    let runner = FakeRunner::new(true, outcome);
    let tool = DelegateTaskTool::new(runner);

    let out = tool
        .execute(&ctx(), ToolArgs(serde_json::json!({ "prompt": "go" })))
        .await
        .unwrap();

    assert!(!out.ok, "a failed child is not a successful call");
    assert!(out.content.contains("model refused"), "{}", out.content);
}

#[tokio::test]
async fn an_aborted_child_still_returns_what_it_produced() {
    let mut outcome = completed("partial findings");
    outcome.status = ark_ports::child_status::ABORTED.to_string();
    let runner = FakeRunner::new(true, outcome);
    let tool = DelegateTaskTool::new(runner);

    let out = tool
        .execute(&ctx(), ToolArgs(serde_json::json!({ "prompt": "go" })))
        .await
        .unwrap();

    assert!(out.ok, "an aborted run's partial answer is still usable");
    assert_eq!(out.content, "partial findings");
}

#[tokio::test]
async fn the_detail_line_carries_identity_and_real_usage() {
    let mut outcome = completed("done");
    outcome.tool_invocations = 4;
    outcome.model = Some("m1".to_string());
    outcome.provider_id = Some("p1".to_string());
    outcome.usage = Usage::new(10, 5);
    let child = outcome.child_id;
    let runner = FakeRunner::new(true, outcome);
    let tool = DelegateTaskTool::new(runner);

    let out = tool
        .execute(&ctx(), ToolArgs(serde_json::json!({ "prompt": "go" })))
        .await
        .unwrap();

    let detail = out.detail.expect("detail must be present");
    assert!(detail.contains(&format!("child={child}")), "{detail}");
    assert!(detail.contains("status=completed"), "{detail}");
    assert!(detail.contains("tools=4"), "{detail}");
    assert!(detail.contains("model=m1"), "{detail}");
    assert!(detail.contains("provider=p1"), "{detail}");
    assert!(detail.contains("tokens=10/5/15"), "{detail}");
}

#[tokio::test]
async fn zero_usage_is_omitted_instead_of_reported_as_zero_tokens() {
    // Reporting `tokens=0/0/0` would look like a real measurement of a free call.
    let runner = FakeRunner::new(true, completed("done"));
    let tool = DelegateTaskTool::new(runner);

    let out = tool
        .execute(&ctx(), ToolArgs(serde_json::json!({ "prompt": "go" })))
        .await
        .unwrap();

    let detail = out.detail.unwrap();
    assert!(!detail.contains("tokens="), "{detail}");
}

#[test]
fn the_schema_requires_a_prompt_and_no_policy_fields() {
    let schema = schema::delegate_task();
    assert_eq!(schema["required"], serde_json::json!(["prompt"]));
    let properties = schema["properties"].as_object().unwrap();
    assert!(properties.contains_key("prompt"));
    assert_eq!(properties["background"]["type"], "boolean");
    assert_eq!(properties["resumeChildId"]["type"], "string");
    assert!(!schema["required"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("resumeChildId")));
    // `workspace` must not be a delegation argument: the child inherits it.
    for forbidden in [
        "workspace",
        "max_depth",
        "max_concurrency",
        "max_queued",
        "timeout_ms",
    ] {
        assert!(
            !properties.contains_key(forbidden),
            "`{forbidden}` must not be a delegation argument"
        );
    }
}

#[test]
fn the_tool_declares_the_contract_name() {
    let runner = FakeRunner::new(true, completed("x"));
    let tool = DelegateTaskTool::new(runner);
    assert_eq!(tool.name(), "delegate_task");
    assert_eq!(tool.schema(), schema::delegate_task());
}
