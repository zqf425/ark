//! Runtime configuration snapshot (design §7.15.1 / §7.15.2).
//!
//! The reload class of every field is declared next to the field itself, and
//! [`RuntimeConfig::apply_patch`] is the only function that turns a
//! [`ConfigPatch`] into a new snapshot. Validation happens on the *candidate*
//! value, so a rejected patch cannot leave the live snapshot half updated.

use ark_contract::{
    ConfigPatch, FieldPath, SubagentSettings, SUBAGENT_CONCURRENCY_MAX, SUBAGENT_CONCURRENCY_MIN,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Log levels accepted by `RUST_LOG`-style filters.
pub const LOG_LEVELS: [&str; 6] = ["trace", "debug", "info", "warn", "error", "off"];

/// Loop and boundary tuning. Every field here is `Hot` (design §7.15.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TuningConfig {
    /// Maximum model round-trips in one turn.
    pub max_rounds: u32,
    /// Maximum cumulative tokens in one turn.
    pub max_tokens: u64,
    /// Maximum tool calls in one turn.
    pub max_tool_calls: u32,
    /// A call whose signature repeats this often is suppressed.
    pub max_identical_calls: u32,
    /// Prompt estimate that triggers compaction.
    pub context_limit_tokens: u64,
    /// Messages kept verbatim when compacting.
    pub keep_recent_messages: usize,
    /// Model-delta channel capacity per round.
    pub chunk_buffer: usize,
    /// SSE keep-alive comment interval, in seconds.
    pub sse_heartbeat_secs: u64,
    /// SSE idle timeout before `sse_idle_timeout`, in seconds (0 disables).
    pub sse_idle_timeout_secs: u64,
}

impl Default for TuningConfig {
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

impl TuningConfig {
    /// Clamp every field into a range that keeps the runtime usable.
    pub fn sanitized(mut self) -> Self {
        self.max_rounds = self.max_rounds.clamp(1, 200);
        self.max_tokens = self.max_tokens.clamp(1_000, 50_000_000);
        self.max_tool_calls = self.max_tool_calls.clamp(1, 5_000);
        self.max_identical_calls = self.max_identical_calls.clamp(1, 100);
        self.context_limit_tokens = self.context_limit_tokens.clamp(1_000, 10_000_000);
        self.keep_recent_messages = self.keep_recent_messages.clamp(1, 10_000);
        self.chunk_buffer = self.chunk_buffer.clamp(1, 4_096);
        self.sse_heartbeat_secs = self.sse_heartbeat_secs.clamp(0, 3_600);
        self.sse_idle_timeout_secs = self.sse_idle_timeout_secs.clamp(0, 86_400);
        self
    }
}

/// The complete runtime configuration.
///
/// `port` and `data_dir` are `Restart` fields: they are part of the snapshot so
/// [`crate::restart_fields`] can name them, but a patch touching them is
/// rejected as a whole batch and never applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    pub port: u16,
    pub data_dir: PathBuf,
    pub log_level: String,
    /// Opaque provider table: `ark-llm` owns its shape, config only stores it.
    pub model_providers: serde_json::Value,
    /// Default workspace for tools. `None` means "no default chosen", which is
    /// different from an empty path: the GUI shows an explicit empty state.
    pub default_workspace: Option<PathBuf>,
    /// Sub-agent switch and concurrency cap (design §7.8).
    pub subagents: SubagentSettings,
    pub tuning: TuningConfig,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            port: super::DEFAULT_PORT,
            data_dir: PathBuf::new(),
            log_level: "info".to_string(),
            model_providers: serde_json::Value::Null,
            default_workspace: None,
            subagents: SubagentSettings::default(),
            tuning: TuningConfig::default(),
        }
    }
}

impl RuntimeConfig {
    /// Reject values that would break the process if they were applied.
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("port 0 is not a listenable port".to_string());
        }
        if !LOG_LEVELS.contains(&self.log_level.as_str()) {
            return Err(format!(
                "unknown log level `{}` (expected one of {})",
                self.log_level,
                LOG_LEVELS.join(", ")
            ));
        }
        if !self.model_providers.is_null()
            && !self.model_providers.is_object()
            && !self.model_providers.is_array()
        {
            return Err("model_providers must be an object or an array".to_string());
        }
        if let Some(workspace) = &self.default_workspace {
            if workspace.as_os_str().is_empty() {
                return Err("default_workspace must not be an empty path".to_string());
            }
        }
        if self.subagents.max_concurrency < SUBAGENT_CONCURRENCY_MIN
            || self.subagents.max_concurrency > SUBAGENT_CONCURRENCY_MAX
        {
            return Err(format!(
                "subagents.max_concurrency must be between {SUBAGENT_CONCURRENCY_MIN} and {SUBAGENT_CONCURRENCY_MAX}"
            ));
        }
        let tuning = self.tuning.clone().sanitized();
        if tuning != self.tuning {
            return Err("tuning contains an out-of-range value".to_string());
        }
        Ok(())
    }

    /// Build the candidate snapshot for `patch`, or explain why it is invalid.
    ///
    /// No mutation happens here: the caller swaps the snapshot only after the
    /// candidate validates, which is what makes the update transactional.
    pub fn candidate(&self, patch: &ConfigPatch) -> Result<(Self, Vec<FieldPath>), String> {
        let mut next = self.clone();
        let mut changed = Vec::new();

        if let Some(level) = &patch.log_level {
            if next.log_level != *level {
                next.log_level = level.clone();
                changed.push(FieldPath::new("log_level"));
            }
        }
        if let Some(value) = &patch.tuning {
            let tuning: TuningConfig = serde_json::from_value(value.clone())
                .map_err(|err| format!("tuning is not a valid object: {err}"))?;
            if next.tuning != tuning {
                next.tuning = tuning;
                changed.push(FieldPath::new("tuning"));
            }
        }
        if let Some(value) = &patch.model_providers {
            if next.model_providers != *value {
                next.model_providers = value.clone();
                changed.push(FieldPath::new("model_providers"));
            }
        }
        if let Some(value) = &patch.default_workspace {
            // An empty string is the explicit "clear the default" instruction,
            // which is why it is normalized to `None` instead of an empty path.
            let candidate = if value.trim().is_empty() {
                None
            } else {
                Some(PathBuf::from(value.trim()))
            };
            if next.default_workspace != candidate {
                next.default_workspace = candidate;
                changed.push(FieldPath::new("default_workspace"));
            }
        }
        if let Some(value) = &patch.subagents {
            if next.subagents != *value {
                next.subagents = *value;
                changed.push(FieldPath::new("subagents"));
            }
        }
        next.validate()?;
        Ok((next, changed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_log_levels_pass_and_unknown_ones_fail() {
        let mut config = RuntimeConfig::default();
        assert!(config.validate().is_ok());
        config.log_level = "chatty".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn out_of_range_tuning_is_rejected() {
        let mut config = RuntimeConfig::default();
        config.tuning.max_rounds = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn candidate_reports_only_fields_that_changed() {
        let config = RuntimeConfig::default();
        let patch = ConfigPatch {
            log_level: Some("debug".into()),
            ..ConfigPatch::default()
        };
        let (next, changed) = config.candidate(&patch).unwrap();
        assert_eq!(next.log_level, "debug");
        assert_eq!(changed, vec![FieldPath::new("log_level")]);
    }

    #[test]
    fn empty_patch_changes_nothing() {
        let config = RuntimeConfig::default();
        let (next, changed) = config.candidate(&ConfigPatch::default()).unwrap();
        assert_eq!(next, config);
        assert!(changed.is_empty());
    }
}
