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
    /// Skills discovery switch and extra scan roots (docs/modules/ark-skills.md).
    #[serde(default)]
    pub skills: Option<SkillsSettings>,
    /// Agent lifecycle hooks (requirements/hooks.md). Hot: the next tool call
    /// reads the new snapshot.
    #[serde(default)]
    pub hooks: Option<HookSettings>,
    /// Scheduled-task master switch (requirements/scheduled-tasks.md).
    #[serde(default)]
    pub schedules: Option<ScheduleSettings>,
    /// IM-channel master switch (requirements/im-channels.md).
    #[serde(default)]
    pub im: Option<ImSettings>,
}

/// Scheduled-task policy (requirements/scheduled-tasks.md).
///
/// Only the master switch is configuration: the tasks themselves live in
/// `<data_dir>/schedules.json`, because their run bookkeeping changes on every
/// tick, while a config patch is a rarer, validated, versioned edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ScheduleSettings {
    /// When `false` the tick loop keeps running but fires nothing; the task
    /// table stays readable and editable, it just triggers no run.
    pub enabled: bool,
}

impl Default for ScheduleSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// IM-channel policy (requirements/im-channels.md).
///
/// Only the master switch is configuration: channels, credentials and bindings
/// live in `<data_dir>/im.json` (same reasoning as `schedules.json`), and the
/// switch is `Hot` — the IM service reconciles connections against the live
/// snapshot.
///
/// The default is `false`, deliberately the opposite of schedules: an IM
/// channel triggers unattended turns that execute tools on this machine, so
/// the feature must be switched on explicitly rather than light up whatever
/// channels were configured earlier.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ImSettings {
    /// When `false` the IM service keeps channels editable but tears down and
    /// refuses to start their connections.
    pub enabled: bool,
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
    /// Sliding window, in model rounds, the identical-call counter looks at.
    /// `0` counts the whole turn — the historical behaviour.
    #[ts(as = "f64")]
    pub tool_storm_window_size: u32,
    /// Maximum tool calls executed concurrently inside one read-only batch.
    /// `0` means no cap — the historical behaviour.
    #[ts(as = "f64")]
    pub max_parallel_tool_calls: u32,
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
            tool_storm_window_size: 0,
            max_parallel_tool_calls: 0,
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

/// Skills discovery policy (docs/modules/ark-skills.md).
///
/// `enabled: false` empties every skills view. `extra_dirs` are additional
/// scan roots — absolute or `~`-prefixed paths; the workspace-relative
/// directories the runtime already scans are not addressable here, because a
/// workspace-relative config entry would silently change meaning when the
/// workspace switches. Restart-classified: the skill runtime snapshots this
/// at process start, so a patch reports `restart_required`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct SkillsSettings {
    pub enabled: bool,
    /// Always serialized: the settings view reads it as a list, and a missing
    /// key would force every consumer into `?? []` fallbacks.
    #[serde(default)]
    pub extra_dirs: Vec<String>,
}

/// Bounds on `extra_dirs`. Beyond 16 roots the per-request rescan cost and the
/// instruction budget stop being predictable, so the ceiling is enforced.
pub const SKILLS_EXTRA_DIRS_MAX: usize = 16;
pub const SKILLS_EXTRA_DIR_MAX_CHARS: usize = 512;

impl Default for SkillsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_dirs: Vec::new(),
        }
    }
}

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

/// One lifecycle hook: a shell command the runtime executes at a defined point
/// (requirements/hooks.md). The command is trusted configuration, not model
/// input; the payload describing the action arrives on stdin as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct HookRule {
    pub command: String,
    /// Restrict the rule to these tool names; empty matches every tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// Hard timeout in milliseconds; `0` means the runtime default (10s).
    #[serde(default)]
    #[ts(as = "f64")]
    pub timeout_ms: u64,
}

/// Hooks attached to the two v1 lifecycle points.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct HookSettings {
    /// Run before a tool dispatches; exit code 2 blocks the call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre_tool_use: Vec<HookRule>,
    /// Run after a tool completes; stdout is appended to the tool output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_tool_use: Vec<HookRule>,
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
    pub skills: SkillsSettings,
    pub schedules: ScheduleSettings,
    /// Always serialized, including on older readers that default it: the
    /// renderer treats a missing key the same as `enabled: false`.
    #[serde(default)]
    pub im: ImSettings,
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
    fn schedules_are_enabled_by_default_and_fold_legacy_json() {
        assert!(ScheduleSettings::default().enabled);
        // A settings document written before the switch existed must not
        // disable the feature on load.
        let patch: ConfigPatch = serde_json::from_str(r#"{}"#).unwrap();
        assert!(patch.schedules.is_none());
        let parsed: ScheduleSettings = serde_json::from_str(r#"{"enabled":false}"#).unwrap();
        assert!(!parsed.enabled);
    }

    #[test]
    fn auto_continue_is_disabled_by_default_and_for_legacy_json() {
        assert!(!SubagentSettings::default().auto_continue);
        let settings: SubagentSettings =
            serde_json::from_str(r#"{"enabled":true,"max_concurrency":2}"#).unwrap();
        assert!(!settings.auto_continue);
    }
}
