//! Config typing and reload classification (design §7.15).
//!
//! The reload class is part of the type, not a documentation convention: a new
//! field must choose one, and `Restart` patches are rejected as a whole batch.

use serde::{Deserialize, Serialize};

/// How a config change takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum ReloadClass {
    /// Applied in place, no restart, no dropped connection.
    Hot,
    /// Applied in place but listeners are rebuilt (TLS rotation): SSE blips once
    /// and resumes from `since_seq`.
    HotRelisten,
    /// Requires restarting `arkd`.
    Restart,
}

/// Dotted path of a config field, e.g. `server.port`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(transparent)]
pub struct FieldPath(pub String);

impl FieldPath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }
}

impl std::fmt::Display for FieldPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A config patch. `Restart` fields make the whole patch fail; there is no
/// partial application and no silent ignore.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ConfigPatch {
    #[serde(default)]
    pub model_providers: Option<serde_json::Value>,
    #[serde(default)]
    pub tuning: Option<TuningSettings>,
    #[serde(default)]
    pub log_level: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    /// Absolute path of the workspace tools operate in. An empty string clears
    /// the setting back to "no default".
    #[serde(default)]
    pub default_workspace: Option<String>,
    /// Default sandbox autonomy for threads without an explicit override.
    #[serde(default)]
    pub default_autonomy: Option<String>,
    /// Sub-agent switch plus the concurrency cap.
    #[serde(default)]
    pub subagents: Option<SubagentSettings>,
}

/// Hot runtime limits exposed by the settings screen.
///
/// This is a wire DTO, not the runtime's policy implementation. Keeping it
/// typed makes the Rust contract generate the only renderer shape instead of
/// leaving the UI to recreate a loosely typed JSON object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TuningSettings {
    #[ts(as = "f64")]
    pub max_rounds: u32,
    #[ts(as = "f64")]
    pub max_tokens: u64,
    #[ts(as = "f64")]
    pub max_tool_calls: u32,
    #[ts(as = "f64")]
    pub max_identical_calls: u32,
    #[ts(as = "f64")]
    pub context_limit_tokens: u64,
    #[ts(as = "f64")]
    pub keep_recent_messages: usize,
    #[ts(as = "f64")]
    pub chunk_buffer: usize,
    #[ts(as = "f64")]
    pub sse_heartbeat_secs: u64,
    #[ts(as = "f64")]
    pub sse_idle_timeout_secs: u64,
}

impl Default for TuningSettings {
    fn default() -> Self {
        Self {
            max_rounds: 24,
            max_tokens: 1_000_000,
            max_tool_calls: 200,
            max_identical_calls: 3,
            context_limit_tokens: 96_000,
            keep_recent_messages: 12,
            chunk_buffer: 64,
            sse_heartbeat_secs: 15,
            sse_idle_timeout_secs: 300,
        }
    }
}

/// Sub-agent policy (design §7.8 / §7.14).
///
/// `max_concurrency` is the number of child runs that may be in flight at once.
/// It is enforced by a real gate, so raising it above the clamp is rejected
/// instead of silently ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct SubagentSettings {
    pub enabled: bool,
    #[ts(as = "f64")]
    pub max_concurrency: u32,
    /// Start a parent continuation after background child completions.
    /// Disabled by default because enabling it permits additional model calls.
    #[serde(default)]
    pub auto_continue: bool,
}

/// Hard bounds on `max_concurrency`. Above 8 the local model budget and the
/// event log both degrade, so the ceiling is enforced rather than advisory.
pub const SUBAGENT_CONCURRENCY_MIN: u32 = 1;
pub const SUBAGENT_CONCURRENCY_MAX: u32 = 8;

impl Default for SubagentSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_concurrency: 2,
            auto_continue: false,
        }
    }
}

impl SubagentSettings {
    pub fn sanitized(self) -> Self {
        Self {
            enabled: self.enabled,
            max_concurrency: self
                .max_concurrency
                .clamp(SUBAGENT_CONCURRENCY_MIN, SUBAGENT_CONCURRENCY_MAX),
            auto_continue: self.auto_continue,
        }
    }
}

/// `GET /v1/runtime/config` — what the settings screen renders.
///
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct RuntimeConfigView {
    #[ts(as = "f64")]
    pub version: u64,
    pub port: u16,
    pub data_dir: String,
    pub log_level: String,
    /// Always serialized, including `null`: the renderer's contract is
    /// `string | null`, and omitting the key would make it `undefined` instead.
    pub default_workspace: Option<String>,
    pub default_autonomy: String,
    pub subagents: SubagentSettings,
    pub tuning: TuningSettings,
}

/// Outcome of `POST /v1/runtime/config`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ApplyResult {
    Applied {
        #[ts(as = "f64")]
        version: u64,
        changed: Vec<FieldPath>,
    },
    Rejected {
        restart_required: Vec<FieldPath>,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_carries_field_paths() {
        let r = ApplyResult::Rejected {
            restart_required: vec![FieldPath::new("server.port")],
            reason: "restart required".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"result\":\"rejected\""));
        assert!(json.contains("server.port"));
    }

    #[test]
    fn auto_continue_is_disabled_by_default_and_for_legacy_json() {
        assert!(!SubagentSettings::default().auto_continue);
        let settings: SubagentSettings =
            serde_json::from_str(r#"{"enabled":true,"max_concurrency":2}"#).unwrap();
        assert!(!settings.auto_continue);
    }
}
