//! Reload classification table (design §7.15.2).
//!
//! The class is looked up from the field path, so adding a config field forces
//! a deliberate classification decision instead of an implicit default.

use ark_contract::{ConfigPatch, FieldPath, ReloadClass};

use super::snapshot::TuningConfig;

/// Every field path the configuration owns, with its reload class.
pub const FIELD_CLASSES: &[(&str, ReloadClass)] = &[
    ("port", ReloadClass::Restart),
    ("data_dir", ReloadClass::Restart),
    ("log_level", ReloadClass::Hot),
    ("model_providers", ReloadClass::HotRelisten),
    ("default_workspace", ReloadClass::Hot),
    ("subagents", ReloadClass::Hot),
    ("tuning", ReloadClass::Hot),
];

/// Field paths nested under `tuning`. Each one is `Hot`.
pub const TUNING_FIELDS: &[&str] = &[
    "tuning.max_rounds",
    "tuning.max_tokens",
    "tuning.max_tool_calls",
    "tuning.max_identical_calls",
    "tuning.context_limit_tokens",
    "tuning.keep_recent_messages",
    "tuning.chunk_buffer",
    "tuning.sse_heartbeat_secs",
    "tuning.sse_idle_timeout_secs",
];

/// Class of one field path. Unknown paths are treated as `Restart`, so a typo
/// cannot silently hot-apply something nobody classified.
pub fn reload_class(path: &str) -> ReloadClass {
    FIELD_CLASSES
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, class)| *class)
        .unwrap_or(ReloadClass::Restart)
}

/// `Restart` field paths present in `patch`, in table order.
pub fn restart_fields(patch: &ConfigPatch) -> Vec<FieldPath> {
    let mut out = Vec::new();
    if patch.port.is_some() {
        out.push(FieldPath::new("port"));
    }
    out
}

/// Passes that only take effect after a listener rebuild.
pub fn relisten_fields(patch: &ConfigPatch) -> Vec<FieldPath> {
    let mut out = Vec::new();
    if patch.model_providers.is_some() {
        out.push(FieldPath::new("model_providers"));
    }
    out
}

/// Flatten a tuning struct into `(path, value)` pairs, so a diff can name the
/// exact nested field instead of just `tuning`.
pub fn tuning_diff(old: &TuningConfig, new: &TuningConfig) -> Vec<FieldPath> {
    let mut out = Vec::new();
    if old.max_rounds != new.max_rounds {
        out.push(FieldPath::new("tuning.max_rounds"));
    }
    if old.max_tokens != new.max_tokens {
        out.push(FieldPath::new("tuning.max_tokens"));
    }
    if old.max_tool_calls != new.max_tool_calls {
        out.push(FieldPath::new("tuning.max_tool_calls"));
    }
    if old.max_identical_calls != new.max_identical_calls {
        out.push(FieldPath::new("tuning.max_identical_calls"));
    }
    if old.context_limit_tokens != new.context_limit_tokens {
        out.push(FieldPath::new("tuning.context_limit_tokens"));
    }
    if old.keep_recent_messages != new.keep_recent_messages {
        out.push(FieldPath::new("tuning.keep_recent_messages"));
    }
    if old.chunk_buffer != new.chunk_buffer {
        out.push(FieldPath::new("tuning.chunk_buffer"));
    }
    if old.sse_heartbeat_secs != new.sse_heartbeat_secs {
        out.push(FieldPath::new("tuning.sse_heartbeat_secs"));
    }
    if old.sse_idle_timeout_secs != new.sse_idle_timeout_secs {
        out.push(FieldPath::new("tuning.sse_idle_timeout_secs"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_fields_are_classified_as_restart() {
        assert_eq!(reload_class("port"), ReloadClass::Restart);
        assert_eq!(reload_class("log_level"), ReloadClass::Hot);
        assert_eq!(reload_class("model_providers"), ReloadClass::HotRelisten);
    }

    #[test]
    fn unknown_paths_default_to_restart() {
        assert_eq!(reload_class("nope"), ReloadClass::Restart);
    }

    #[test]
    fn restart_patch_fields_are_listed() {
        let patch = ConfigPatch {
            port: Some(19000),
            ..ConfigPatch::default()
        };
        assert_eq!(restart_fields(&patch), vec![FieldPath::new("port")]);
    }

    #[test]
    fn tuning_diff_names_nested_fields() {
        let old = TuningConfig::default();
        let mut new = old.clone();
        new.max_rounds = 3;
        assert_eq!(
            tuning_diff(&old, &new),
            vec![FieldPath::new("tuning.max_rounds")]
        );
    }
}
