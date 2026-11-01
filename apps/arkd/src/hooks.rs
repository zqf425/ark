//! Agent lifecycle hooks (requirements/hooks.md) — the single execution host.
//!
//! Hooks are trusted configuration, not model input: the command comes from
//! `config.json` / a validated config patch, and the action description arrives
//! on stdin as JSON. Both the parent turn runner and the child runner call this
//! module's combinator, so the pre/post semantics exist exactly once.

use std::path::Path;
use std::time::Duration;

use ark_contract::{HookRule, HookSettings};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use ark_common::Result;
use ark_ports::ToolOutput;

/// Timeout applied when a rule does not set `timeout_ms`.
pub const DEFAULT_HOOK_TIMEOUT_MS: u64 = 10_000;
/// Hard ceiling for one hook invocation.
pub const MAX_HOOK_TIMEOUT_MS: u64 = 60_000;
/// Cap on hook text that reaches the model (block reason or post note).
const HOOK_TEXT_CAP_BYTES: usize = 4 * 1024;

/// Run the pre/post hooks around one tool dispatch.
///
/// `dispatch` is the inner execution; a blocked call never reaches it, but the
/// caller still records the refusal as the tool's result, so the reactor's
/// started/finished event pairing stays intact.
pub async fn run_around<F, Fut>(
    hooks: &HookSettings,
    workspace: &Path,
    payload: &Value,
    dispatch: F,
) -> Result<ToolOutput>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<ToolOutput>>,
{
    if let Some(reason) = run_pre_hooks(&hooks.pre_tool_use, payload, workspace).await {
        return Ok(ToolOutput {
            ok: false,
            content: reason,
            detail: Some("blocked_by_hook".to_string()),
        });
    }
    let mut output = dispatch().await?;
    if let Some(note) = run_post_hooks(&hooks.post_tool_use, payload, workspace).await {
        output.content.push_str("\n[post_tool_use hook]\n");
        output.content.push_str(&note);
    }
    Ok(output)
}

/// Execute `pre_tool_use` rules in order. `Some(reason)` blocks the call.
///
/// Exit code 2 is the only blocking outcome; anything else (other codes,
/// crashes, timeouts, spawn failures) logs a warning and lets the call proceed,
/// because a broken hook must not silently paralyse every tool call.
async fn run_pre_hooks(rules: &[HookRule], payload: &Value, workspace: &Path) -> Option<String> {
    let tool = payload.get("tool").and_then(Value::as_str)?;
    for rule in rules {
        if !rule_matches(rule, tool) {
            continue;
        }
        match invoke(rule, payload, workspace).await {
            Ok(output) if output.code == 2 => {
                let reason = if output.stderr.trim().is_empty() {
                    format!("tool `{tool}` was blocked by a pre_tool_use hook")
                } else {
                    cap_text(output.stderr.trim())
                };
                return Some(reason);
            }
            Ok(output) if output.code != 0 => {
                tracing::warn!(
                    tool,
                    code = output.code,
                    "pre_tool_use hook exited non-zero; continuing"
                );
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(tool, %error, "pre_tool_use hook failed to run; continuing");
            }
        }
    }
    None
}

/// Execute `post_tool_use` rules in order, collecting their stdout as a note.
async fn run_post_hooks(rules: &[HookRule], payload: &Value, workspace: &Path) -> Option<String> {
    let tool = payload.get("tool").and_then(Value::as_str)?;
    let mut notes = String::new();
    for rule in rules {
        if !rule_matches(rule, tool) {
            continue;
        }
        match invoke(rule, payload, workspace).await {
            Ok(output) if output.code == 0 && !output.stdout.trim().is_empty() => {
                if !notes.is_empty() {
                    notes.push('\n');
                }
                notes.push_str(&cap_text(output.stdout.trim()));
            }
            Ok(output) if output.code != 0 => {
                tracing::warn!(
                    tool,
                    code = output.code,
                    "post_tool_use hook exited non-zero; ignored"
                );
            }
            _ => {}
        }
    }
    (!notes.is_empty()).then_some(notes)
}

/// Whether a rule applies to this tool (empty filter = every tool).
fn rule_matches(rule: &HookRule, tool: &str) -> bool {
    rule.tools.is_empty() || rule.tools.iter().any(|name| name == tool)
}

struct HookProcessOutput {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Spawn one hook command with the payload on stdin.
async fn invoke(rule: &HookRule, payload: &Value, workspace: &Path) -> Result<HookProcessOutput> {
    let timeout = Duration::from_millis(if rule.timeout_ms == 0 {
        DEFAULT_HOOK_TIMEOUT_MS
    } else {
        rule.timeout_ms.min(MAX_HOOK_TIMEOUT_MS)
    });
    let mut command = shell(&rule.command);
    // All three pipes: payload in, decision text out. `wait_with_output` only
    // captures streams that are piped, so an inherit default would lose both.
    command
        .current_dir(workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|err| ark_common::ArkError::internal(format!("hook could not start: {err}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let body = serde_json::to_string(payload).unwrap_or_default();
        // A hook that never reads stdin must not deadlock us: the write races
        // the wait and a closed pipe is harmless.
        let _ = tokio::time::timeout(timeout / 2, stdin.write_all(body.as_bytes())).await;
    }
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| ark_common::ArkError::internal(format!("hook timed out after {timeout:?}")))?
        .map_err(|err| ark_common::ArkError::internal(format!("hook failed: {err}")))?;
    Ok(HookProcessOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// Platform-appropriate shell for trusted commands (mirrors the bash tool).
#[cfg(windows)]
fn shell(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd
}

#[cfg(not(windows))]
fn shell(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg("-c").arg(command);
    cmd
}

/// Cut hook text that reaches the model on a character boundary.
fn cap_text(text: &str) -> String {
    if text.len() <= HOOK_TEXT_CAP_BYTES {
        return text.to_string();
    }
    let mut end = HOOK_TEXT_CAP_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[hook output truncated]", &text[..end])
}

/// The stdin payload for one tool call (requirements/hooks.md §3).
pub fn hook_payload(
    phase: &str,
    tool: &str,
    arguments: &Value,
    thread_id: &ark_common::ThreadId,
    turn_id: &ark_common::TurnId,
) -> Value {
    json!({
        "phase": phase,
        "tool": tool,
        "arguments": arguments,
        "thread_id": thread_id.to_string(),
        "turn_id": turn_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::HookRule;

    fn rule(command: &str) -> HookRule {
        HookRule {
            command: command.into(),
            tools: Vec::new(),
            timeout_ms: 0,
        }
    }

    fn payload(tool: &str) -> Value {
        json!({
            "phase": "pre_tool_use",
            "tool": tool,
            "arguments": {"path": "a.txt"},
            "thread_id": "t",
            "turn_id": "u",
        })
    }

    fn settings(pre: Vec<HookRule>, post: Vec<HookRule>) -> HookSettings {
        HookSettings {
            pre_tool_use: pre,
            post_tool_use: post,
        }
    }

    async fn dispatch_ok() -> Result<ToolOutput> {
        Ok(ToolOutput {
            ok: true,
            content: "inner".into(),
            detail: None,
        })
    }

    #[tokio::test]
    async fn exit_zero_allows_the_call() {
        let hooks = settings(vec![rule("true")], vec![]);
        let out = run_around(&hooks, Path::new("."), &payload("read"), dispatch_ok)
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.content, "inner");
    }

    #[tokio::test]
    async fn exit_two_blocks_with_stderr_as_the_reason() {
        let hooks = settings(vec![rule("echo denied >&2; exit 2")], vec![]);
        let out = run_around(&hooks, Path::new("."), &payload("read"), dispatch_ok)
            .await
            .unwrap();
        assert!(!out.ok);
        assert_eq!(out.content, "denied");
        assert_eq!(out.detail.as_deref(), Some("blocked_by_hook"));
    }

    #[tokio::test]
    async fn other_exit_codes_and_timeouts_still_allow() {
        let hooks = settings(vec![rule("exit 3"), rule("sleep 5"), rule("false")], vec![]);
        let out = run_around(&hooks, Path::new("."), &payload("read"), dispatch_ok)
            .await
            .unwrap();
        assert!(out.ok);
    }

    #[tokio::test]
    async fn a_timed_out_pre_hook_is_not_a_block() {
        let mut fast = rule("true");
        fast.timeout_ms = 100;
        let mut slow = rule("sleep 3");
        slow.timeout_ms = 100;
        let hooks = settings(vec![fast, slow], vec![]);
        let out = run_around(&hooks, Path::new("."), &payload("read"), dispatch_ok)
            .await
            .unwrap();
        assert!(out.ok);
    }

    #[tokio::test]
    async fn post_hook_stdout_is_appended_to_the_output() {
        let hooks = settings(vec![], vec![rule("echo 'formatted 2 files'")]);
        let out = run_around(&hooks, Path::new("."), &payload("write"), dispatch_ok)
            .await
            .unwrap();
        assert!(out.content.contains("[post_tool_use hook]"));
        assert!(out.content.contains("formatted 2 files"));
    }

    #[tokio::test]
    async fn the_tool_filter_skips_unmatched_rules() {
        let mut scoped = rule("exit 2");
        scoped.tools = vec!["bash".into()];
        let hooks = settings(vec![scoped], vec![]);
        let out = run_around(&hooks, Path::new("."), &payload("read"), dispatch_ok)
            .await
            .unwrap();
        assert!(out.ok, "a rule scoped to bash must not gate `read`");
    }

    #[test]
    fn hook_text_is_capped_on_a_character_boundary() {
        let long = "汉".repeat(HOOK_TEXT_CAP_BYTES / 3 + 10);
        let capped = cap_text(&long);
        assert!(capped.len() < long.len());
        assert!(capped.ends_with("[hook output truncated]"));
    }
}
