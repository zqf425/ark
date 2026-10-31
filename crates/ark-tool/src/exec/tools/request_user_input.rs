//! `request_user_input` facade tool (design §7.3).
//!
//! The tool only exists so the model can *express* the question. The reactor
//! intercepts the call in `settle_model` — before any dispatch plan is built —
//! and routes it through the user-input gate, so `execute` is unreachable in
//! production. If it ever runs (a future refactor lost the interception, a
//! registry handed the tool to an executor that skips the reactor), the error
//! must say exactly that instead of silently succeeding.

use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput};

/// `request_user_input`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestUserInputTool;

#[async_trait::async_trait]
impl Tool for RequestUserInputTool {
    fn name(&self) -> &'static str {
        "request_user_input"
    }

    fn schema(&self) -> &'static serde_json::Value {
        crate::domain::schema::request_user_input()
    }

    async fn execute(&self, _ctx: &ToolContext, _raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        Ok(ToolOutput::err(
            "request_user_input must be intercepted by the agent's user-input gate before \
             dispatch; it cannot execute as a normal tool",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_matches_the_prompt_contract() {
        let schema = RequestUserInputTool.schema();
        assert_eq!(schema["required"][0], "prompt");
        assert_eq!(schema["properties"]["prompt"]["type"], "string");
    }

    #[tokio::test]
    async fn execute_refuses_instead_of_succeeding() {
        let ctx = ToolContext {
            thread_id: ark_common::ThreadId::new(),
            turn_id: ark_common::TurnId::new(),
            workspace: std::env::temp_dir(),
            sandbox: ark_ports::SandboxDecision::ReadOnly,
            env: Default::default(),
        };
        let output = RequestUserInputTool
            .execute(&ctx, ToolArgs(serde_json::json!({ "prompt": "hi" })))
            .await
            .unwrap();
        assert!(!output.ok);
        assert!(output.content.contains("gate"));
    }
}
