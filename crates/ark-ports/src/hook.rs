//! Hook bus port (requirements/plugin-hook-bus.md, P1).
//!
//! Hooks and events are two namespaces: `EventKind`, the session log and SSE
//! carry facts that already happened, while hooks are the extension surface a
//! plugin uses to observe (and, from P2 on, narrowly rewrite) those behaviours.
//! Hook payloads therefore never flow into the session log or the SSE stream.
//!
//! The default implementation [`NoHooks`] is a no-op, so a runtime without
//! hook-capable plugins behaves exactly as it did before this port existed;
//! every decorator around it must keep that byte-for-byte promise.

use ark_common::{Result, ThreadId, TurnId, Usage};

use crate::model::ModelRequest;

/// Hook ABI the host speaks. A manifest declaring another value is skipped at
/// plan time (fail closed) instead of being partially honoured.
pub const HOOK_ABI: u32 = 1;

/// Payload budget for the `llm.call` text tail (design: payload table).
pub const LLM_TAIL_BYTES: usize = 2 * 1024;

/// Payload budget for the head of tool-call arguments (design: payload table).
pub const TOOL_ARGS_HEAD_BYTES: usize = 4 * 1024;

/// Which turn a hook fires for. Hooks are always turn-scoped: no hook ever
/// receives a process-wide subscription surface.
#[derive(Debug, Clone, Copy)]
pub struct HookScope<'a> {
    pub thread_id: &'a ThreadId,
    pub turn_id: &'a TurnId,
}

/// Narrow rewrites a `llm.request` hook may return. `None` means "no opinion":
/// the field keeps its configured value. Messages and the tool catalogue are
/// untouchable on purpose — they are the prompt-cache and safety anchors.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelRequestTuning {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

/// Verdict of a synchronous `tool.pre_call` hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolPreCallVerdict {
    /// No opinion: the call proceeds exactly as the reactor decided.
    Allow,
    /// Refuse this call; the reason reaches the model as the tool error.
    Block { reason: String },
}

/// Terminal outcome of one turn, as the supervisor observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcomeFact {
    Completed,
    Failed { message: String },
    Aborted { reason: String },
}

/// Facts of one finished model call. `text_tail` is already truncated to
/// [`LLM_TAIL_BYTES`] with [`truncate_utf8`]; `truncated` tells the receiver
/// the tail is not the whole text so it must not draw conclusions from its
/// apparent end.
#[derive(Debug)]
pub struct LlmCallFact<'a> {
    pub provider_id: &'a str,
    pub model: &'a str,
    pub usage: Usage,
    pub text_tail: &'a str,
    pub truncated: bool,
    /// Set when the call failed; the observe hook then carries no text tail.
    pub error: Option<&'a str>,
}

/// The hook bus port. Production multiplexes authorized plugins behind one
/// implementation; until that exists the composition root installs [`NoHooks`].
///
/// Implementations must never be invoked while the caller holds the workspace
/// gate, the turn registry lock or the event-writer lock: a plugin callback
/// that re-enters the host through those paths would self-deadlock
/// (requirements/plugin-hook-bus.md, Decide 调用纪律).
#[async_trait::async_trait]
pub trait HookBroker: Send + Sync {
    /// ABI version this implementation speaks.
    fn hook_abi(&self) -> u32 {
        HOOK_ABI
    }

    /// `turn.pre_start`: fired before the workspace gate is acquired, so a
    /// denial cannot leave a half-registered turn behind. `Err` refuses the
    /// turn. Callers must surface the error instead of retrying.
    async fn on_turn_pre_start(&self, scope: HookScope<'_>, source: &str) -> Result<()>;

    /// `llm.request`: fired before the model call. `Err` cancels the call
    /// before it is metered or sent.
    async fn on_model_request(
        &self,
        scope: HookScope<'_>,
        request: &ModelRequest,
    ) -> Result<ModelRequestTuning>;

    /// `tool.pre_call`: fired before the shell `pre_tool_use` hook and the
    /// dispatch. `Err` is treated like [`ToolPreCallVerdict::Allow`] by the
    /// caller only when the point is fail-open; the composition root decides.
    async fn on_tool_pre_call(
        &self,
        scope: HookScope<'_>,
        tool: &str,
        args_head: &str,
        args_truncated: bool,
    ) -> Result<ToolPreCallVerdict>;

    /// `turn.completed` / `turn.failed` / `turn.aborted`, fired by the turn
    /// supervisor after the registry settled the turn.
    async fn observe_turn_end(&self, scope: HookScope<'_>, outcome: &TurnOutcomeFact);

    /// `llm.call.completed` / `.failed`, fired after the model call returned.
    async fn observe_llm_call(&self, scope: HookScope<'_>, fact: &LlmCallFact<'_>);
}

/// The no-op broker: every decision is "no opinion" and no observation is
/// recorded. The composition root installs it when no plugin supplies hooks,
/// which is what makes the hook surface opt-in rather than ambient.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHooks;

#[async_trait::async_trait]
impl HookBroker for NoHooks {
    async fn on_turn_pre_start(&self, _scope: HookScope<'_>, _source: &str) -> Result<()> {
        Ok(())
    }

    async fn on_model_request(
        &self,
        _scope: HookScope<'_>,
        _request: &ModelRequest,
    ) -> Result<ModelRequestTuning> {
        Ok(ModelRequestTuning::default())
    }

    async fn on_tool_pre_call(
        &self,
        _scope: HookScope<'_>,
        _tool: &str,
        _args_head: &str,
        _args_truncated: bool,
    ) -> Result<ToolPreCallVerdict> {
        Ok(ToolPreCallVerdict::Allow)
    }

    async fn observe_turn_end(&self, _scope: HookScope<'_>, _outcome: &TurnOutcomeFact) {}

    async fn observe_llm_call(&self, _scope: HookScope<'_>, _fact: &LlmCallFact<'_>) {}
}

/// Truncate to a byte budget without splitting a UTF-8 code point. Returns the
/// truncated slice and whether anything was cut — a payload consumer that
/// mistakes a tail for the whole text makes wrong decisions, so the flag must
/// travel with the slice.
pub fn truncate_utf8(text: &str, limit_bytes: usize) -> (&str, bool) {
    if text.len() <= limit_bytes {
        return (text, false);
    }
    let mut cut = limit_bytes;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (&text[..cut], true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_respects_byte_budget_and_code_points() {
        assert_eq!(truncate_utf8("short", 16), ("short", false));
        let (head, truncated) = truncate_utf8("abcd中文", 6);
        assert_eq!(head, "abcd");
        assert!(truncated);
        // A budget landing inside a multi-byte character cuts before it.
        let (head, truncated) = truncate_utf8("中文字", 4);
        assert_eq!(head, "中");
        assert!(truncated);
    }

    #[tokio::test]
    async fn no_hooks_has_no_opinion_everywhere() {
        let broker = NoHooks;
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let scope = HookScope {
            thread_id: &thread,
            turn_id: &turn,
        };
        assert!(broker.on_turn_pre_start(scope, "user").await.is_ok());
        assert_eq!(
            broker
                .on_model_request(scope, &ModelRequest::default())
                .await
                .unwrap(),
            ModelRequestTuning::default()
        );
        assert_eq!(
            broker
                .on_tool_pre_call(scope, "bash", "{}", false)
                .await
                .unwrap(),
            ToolPreCallVerdict::Allow
        );
        broker
            .observe_turn_end(scope, &TurnOutcomeFact::Completed)
            .await;
        let usage = Usage::default();
        broker
            .observe_llm_call(
                scope,
                &LlmCallFact {
                    provider_id: "p",
                    model: "m",
                    usage,
                    text_tail: "",
                    truncated: false,
                    error: None,
                },
            )
            .await;
        assert_eq!(broker.hook_abi(), HOOK_ABI);
    }
}
