//! Key-level diagnostics for a parsed config document (design §7.15.3).
//!
//! [`RuntimeConfig`](super::RuntimeConfig) accepts partial documents on purpose:
//! a missing field falls back to its default and an unrecognized key is ignored,
//! so a config file written by another tool still starts the runtime instead of
//! refusing to boot. That leniency is only safe when it is *reported*: this
//! module names the keys that were defaulted or dropped, so the loader can warn
//! with the exact paths instead of silently downgrading to defaults.
//!
//! Nothing here performs I/O or changes behavior: it reads a `serde_json::Value`
//! and returns data. `deny_unknown_fields` is deliberately not used — the GUI and
//! `arkd` share the same data directory, so an unknown key must stay tolerated.

use serde_json::{Map, Value};

/// Top-level keys [`RuntimeConfig`](super::RuntimeConfig) understands.
pub const CONFIG_KEYS: &[&str] = &[
    "port",
    "data_dir",
    "log_level",
    "model_providers",
    "default_workspace",
    "default_autonomy",
    "subagents",
    "tuning",
];

/// Keys nested under `subagents`.
pub const SUBAGENT_KEYS: &[&str] = &["enabled", "max_concurrency", "auto_continue"];

/// Keys nested under `tuning`.
pub const TUNING_KEYS: &[&str] = &[
    "max_rounds",
    "max_tokens",
    "max_tool_calls",
    "max_identical_calls",
    "context_limit_tokens",
    "keep_recent_messages",
    "chunk_buffer",
    "sse_heartbeat_secs",
    "sse_idle_timeout_secs",
];

/// Keys that fell back to a default and keys that were ignored, in table order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigDiagnostics {
    missing: Vec<String>,
    unknown: Vec<String>,
}

impl ConfigDiagnostics {
    /// Dotted paths absent from the document; each one keeps its default value.
    pub fn missing(&self) -> &[String] {
        &self.missing
    }

    /// Keys present in the document that this runtime does not understand.
    pub fn unknown(&self) -> &[String] {
        &self.unknown
    }

    /// `true` when the document matched the schema exactly.
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.unknown.is_empty()
    }

    /// One-line summary naming both lists; used by the loader's warning.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.missing.is_empty() {
            parts.push(format!("defaulted=[{}]", self.missing.join(", ")));
        }
        if !self.unknown.is_empty() {
            parts.push(format!("ignored=[{}]", self.unknown.join(", ")));
        }
        parts.join(" ")
    }
}

/// Compare a parsed document against the schema and report every deviation.
///
/// A non-object document yields no diagnostics: `serde` rejects that shape
/// before this function runs, and inventing keys here would produce a warning
/// for a file that never reached the loader.
pub fn diagnose(document: &Value) -> ConfigDiagnostics {
    let mut diagnostics = ConfigDiagnostics::default();
    let Some(object) = document.as_object() else {
        return diagnostics;
    };
    for key in CONFIG_KEYS {
        if !object.contains_key(*key) {
            diagnostics.missing.push((*key).to_string());
        }
    }
    for key in object.keys() {
        if !CONFIG_KEYS.contains(&key.as_str()) {
            diagnostics.unknown.push(key.clone());
        }
    }
    diagnose_nested(object, "subagents", SUBAGENT_KEYS, &mut diagnostics);
    diagnose_nested(object, "tuning", TUNING_KEYS, &mut diagnostics);
    diagnostics
}

/// Report deviations inside one nested table.
///
/// An absent parent is reported once by the top-level pass instead of listing
/// every child, which keeps the warning readable; a parent of the wrong type is
/// a parse error, not a diagnostic.
fn diagnose_nested(
    object: &Map<String, Value>,
    parent: &str,
    keys: &[&str],
    diagnostics: &mut ConfigDiagnostics,
) {
    let Some(Value::Object(nested)) = object.get(parent) else {
        return;
    };
    for key in keys {
        if !nested.contains_key(*key) {
            diagnostics.missing.push(format!("{parent}.{key}"));
        }
    }
    for key in nested.keys() {
        if !keys.contains(&key.as_str()) {
            diagnostics.unknown.push(format!("{parent}.{key}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::RuntimeConfig;

    /// The tables are hand-written, so a new field must fail this test instead
    /// of being silently misreported as unknown at load time.
    #[test]
    fn key_tables_match_the_serialized_schema() {
        let document = serde_json::to_value(RuntimeConfig::default()).expect("config serializes");
        let object = document.as_object().expect("config is an object");

        let mut serialized: Vec<&str> = object.keys().map(String::as_str).collect();
        serialized.sort_unstable();
        let mut declared = CONFIG_KEYS.to_vec();
        declared.sort_unstable();
        assert_eq!(serialized, declared, "CONFIG_KEYS is out of date");

        for (parent, keys) in [("subagents", SUBAGENT_KEYS), ("tuning", TUNING_KEYS)] {
            let nested = object
                .get(parent)
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("{parent} is an object"));
            let mut serialized: Vec<&str> = nested.keys().map(String::as_str).collect();
            serialized.sort_unstable();
            let mut declared = keys.to_vec();
            declared.sort_unstable();
            assert_eq!(serialized, declared, "{parent} key table is out of date");
        }

        assert!(
            diagnose(&document).is_clean(),
            "a complete document must not warn"
        );
    }

    #[test]
    fn gui_shaped_document_reports_every_key_as_defaulted() {
        let document = serde_json::json!({
            "serve": { "port": 18801 },
            "models": [],
            "runtime": { "logLevel": "debug" }
        });
        let diagnostics = diagnose(&document);
        assert_eq!(
            diagnostics.missing(),
            CONFIG_KEYS,
            "every runtime key is absent"
        );
        assert_eq!(diagnostics.unknown(), ["models", "runtime", "serve"]);
        assert!(!diagnostics.is_clean());
    }

    #[test]
    fn absent_nested_table_is_reported_once() {
        let mut document =
            serde_json::to_value(RuntimeConfig::default()).expect("config serializes");
        document
            .as_object_mut()
            .expect("object")
            .remove("subagents");
        let diagnostics = diagnose(&document);
        assert_eq!(diagnostics.missing(), ["subagents"]);
        assert!(diagnostics.unknown().is_empty());
    }

    #[test]
    fn unknown_nested_key_is_named_with_its_parent() {
        let mut document =
            serde_json::to_value(RuntimeConfig::default()).expect("config serializes");
        document["tuning"]["max_steps"] = serde_json::json!(3);
        document["subagents"]["max_concurrency"] = serde_json::json!(4);
        let diagnostics = diagnose(&document);
        assert_eq!(diagnostics.unknown(), ["tuning.max_steps"]);
        assert!(
            diagnostics.missing().is_empty(),
            "a present key is never reported as defaulted"
        );
    }
}
