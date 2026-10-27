//! Config typing and reload classification (design §7.15).
//!
//! The reload class is part of the type, not a documentation convention: a new
//! field must choose one, and `Restart` patches are rejected as a whole batch.

use serde::{Deserialize, Serialize};

/// How a config change takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigPatch {
    #[serde(default)]
    pub model_providers: Option<serde_json::Value>,
    #[serde(default)]
    pub tuning: Option<serde_json::Value>,
    #[serde(default)]
    pub log_level: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    /// Absolute path of the workspace tools operate in. An empty string clears
    /// the setting back to "no default".
    #[serde(default)]
    pub default_workspace: Option<String>,
    /// Sub-agent switch plus the concurrency cap.
    #[serde(default)]
    pub subagents: Option<SubagentSettings>,
}

/// Sub-agent policy (design §7.8 / §7.14).
///
/// `max_concurrency` is the number of child runs that may be in flight at once.
/// It is enforced by a real gate, so raising it above the clamp is rejected
/// instead of silently ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentSettings {
    pub enabled: bool,
    pub max_concurrency: u32,
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
        }
    }
}

/// `GET /v1/runtime/config` — what the settings screen renders.
///
/// `tuning` stays an opaque JSON object on purpose: the settings screen shows it
/// read-only, and duplicating every tuning field here would be a second source
/// of truth for the same numbers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfigView {
    pub version: u64,
    pub port: u16,
    pub data_dir: String,
    pub log_level: String,
    /// Always serialized, including `null`: the renderer's contract is
    /// `string | null`, and omitting the key would make it `undefined` instead.
    pub default_workspace: Option<String>,
    pub subagents: SubagentSettings,
    pub tuning: serde_json::Value,
}

/// Outcome of `POST /v1/runtime/config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ApplyResult {
    Applied {
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
}
