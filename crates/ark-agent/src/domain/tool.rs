//! One tool call as the agent sees it, and the outcome of running it.

use ark_ports::SandboxDecision;
use serde::{Deserialize, Serialize};

/// A complete model-issued tool call. `arguments` is already parsed JSON, so no
/// layer below has to re-parse a string (design §7.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub tool: String,
    pub arguments: serde_json::Value,
    /// The sandbox decision **already made** for this call (design §7.3).
    ///
    /// The reactor consults the policy once, before the gate; the executor then
    /// consumes this value instead of asking the policy again. Recomputing it
    /// meant an approved `bash` still came back as `NeedsApproval` and was
    /// refused after the human had already said yes (P1-5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxDecision>,
}

impl ToolCallRequest {
    pub fn new(
        call_id: impl Into<String>,
        tool: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            tool: tool.into(),
            arguments,
            sandbox: None,
        }
    }

    /// Attach the decision computed for this call.
    pub fn with_sandbox(mut self, decision: SandboxDecision) -> Self {
        self.sandbox = Some(decision);
        self
    }

    /// Stable key for storm detection and parallel-safety: tool name plus the
    /// canonical argument text.
    pub fn signature(&self) -> String {
        format!("{}:{}", self.tool, canonical_json(&self.arguments))
    }

    /// Storm key: the signature with bash commands normalized.
    ///
    /// The breaker compares byte-exact signatures, which a one-token change
    /// defeats while the call keeps repeating the same failed operation. The
    /// observed case: `sleep 300; git push` → `sleep 590; git push` looped for
    /// an hour because every sleep duration produced a fresh signature. For
    /// `bash`, the command is normalized to its first significant verb plus
    /// everything after the leading wait prefixes, so sleep-only retries
    /// collapse onto one key. Other tools keep the exact signature.
    pub fn storm_key(&self) -> String {
        if self.tool != "bash" {
            return self.signature();
        }
        let command = self
            .arguments
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        format!("bash:{}", normalize_bash_command(command))
    }

    /// The resource this call touches, used by the parallel-safety policy.
    pub fn resource(&self) -> Option<&str> {
        // Every session call shares one cwd state machine even though each
        // command owns a process. A fixed resource serialises those calls;
        // plain bash deliberately keeps no resource and remains parallel.
        if self.tool == "bash_session" {
            return Some("shell_session");
        }
        for key in ["path", "file_path", "filePath", "target"] {
            if let Some(value) = self.arguments.get(key).and_then(|v| v.as_str()) {
                return Some(value);
            }
        }
        None
    }
}

/// Deterministic JSON rendering: object keys sorted, no whitespace.
pub fn canonical_json(value: &serde_json::Value) -> String {
    fn walk(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push('{');
                for (idx, key) in keys.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::String((**key).clone()).to_string());
                    out.push(':');
                    walk(&map[*key], out);
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (idx, item) in items.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    walk(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }

    let mut out = String::new();
    walk(value, &mut out);
    out
}

/// Normalize a bash command for storm detection: strip the leading wait
/// prefixes (`sleep N;`, `&&`-joined sleeps) and squeeze whitespace, so a
/// retry that only changes *how long it waits before repeating the same
/// failed operation* maps to the same key. The comparison is intentionally
/// coarse — it bounds a failing loop, it does not fingerprint intent.
pub fn normalize_bash_command(command: &str) -> String {
    let mut rest = command.trim();
    while let Some(semi) = rest.find(';') {
        let head = rest[..semi].trim();
        let seconds = head.strip_prefix("sleep").map(str::trim);
        match seconds {
            Some(value) if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) => {
                rest = rest[semi + 1..].trim_start();
            }
            _ => break,
        }
    }
    rest.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What actually happened when the reactor ran one call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecution {
    pub call_id: String,
    pub tool: String,
    pub ok: bool,
    pub content: String,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ToolExecution {
    pub fn ok(call: &ToolCallRequest, content: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            call_id: call.call_id.clone(),
            tool: call.tool.clone(),
            ok: true,
            content: content.into(),
            duration_ms,
            detail: None,
        }
    }

    pub fn failed(call: &ToolCallRequest, content: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            call_id: call.call_id.clone(),
            tool: call.tool.clone(),
            ok: false,
            content: content.into(),
            duration_ms,
            detail: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_key_order_independent() {
        let a = ToolCallRequest::new(
            "c1",
            "read",
            serde_json::json!({"path": "a.txt", "offset": 3}),
        );
        let b = ToolCallRequest::new(
            "c2",
            "read",
            serde_json::json!({"offset": 3, "path": "a.txt"}),
        );
        assert_eq!(a.signature(), b.signature());
    }

    #[test]
    fn signature_separates_different_tools() {
        let a = ToolCallRequest::new("c1", "read", serde_json::json!({"path": "a.txt"}));
        let b = ToolCallRequest::new("c1", "write", serde_json::json!({"path": "a.txt"}));
        assert_ne!(a.signature(), b.signature());
    }

    #[test]
    fn resource_reads_the_common_argument_names() {
        let call = ToolCallRequest::new("c1", "read", serde_json::json!({"file_path": "x.rs"}));
        assert_eq!(call.resource(), Some("x.rs"));
        let none = ToolCallRequest::new("c2", "list", serde_json::json!({}));
        assert_eq!(none.resource(), None);
    }

    #[test]
    fn session_calls_share_a_resource_but_plain_bash_does_not() {
        let session = ToolCallRequest::new(
            "s1",
            "bash_session",
            serde_json::json!({ "command": "cd src" }),
        );
        let bash = ToolCallRequest::new("b1", "bash", serde_json::json!({ "command": "pwd" }));
        assert_eq!(session.resource(), Some("shell_session"));
        assert_eq!(bash.resource(), None);
    }

    #[test]
    fn storm_key_collapses_sleep_prefixed_bash_retries() {
        // The observed failure shape: the same git push retried behind
        // different sleep durations for an hour, each a fresh byte-exact
        // signature. They must share one storm key.
        let a = ToolCallRequest::new(
            "c1",
            "bash",
            serde_json::json!({"command": "sleep 300; cd repo && git push origin master"}),
        );
        let b = ToolCallRequest::new(
            "c2",
            "bash",
            serde_json::json!({"command": "sleep 590;cd repo && git push origin master"}),
        );
        assert_eq!(a.storm_key(), b.storm_key());
        // Distinct commands stay distinct.
        let c = ToolCallRequest::new(
            "c3",
            "bash",
            serde_json::json!({"command": "sleep 5; git status"}),
        );
        assert_ne!(a.storm_key(), c.storm_key());
    }

    #[test]
    fn non_bash_calls_keep_the_exact_signature_as_storm_key() {
        let a = ToolCallRequest::new("c1", "read", serde_json::json!({"path": "a.txt"}));
        assert_eq!(a.storm_key(), a.signature());
    }

    #[test]
    fn normalize_bash_strips_only_leading_sleeps() {
        assert_eq!(normalize_bash_command("sleep 30; cargo test"), "cargo test");
        assert_eq!(
            normalize_bash_command("  sleep 5;  sleep  10;  git  push "),
            "git push"
        );
        // A non-sleep prefix ends the stripping: `echo sleep` is a command.
        assert_eq!(
            normalize_bash_command("echo sleep; git push"),
            "echo sleep; git push"
        );
        // `sleep` without a number is left in place.
        assert_eq!(
            normalize_bash_command("sleep $T; git push"),
            "sleep $T; git push"
        );
    }
}
