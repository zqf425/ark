//! JSON document persistence for the runtime configuration (design §7.15.3).
//!
//! Three load outcomes are kept distinct on purpose:
//!
//! * **file missing** — normal first run: defaults, no log line;
//! * **parse failure** — an error, because silently resetting a user's config
//!   would be worse than refusing to start;
//! * **partial match** — the document parses but some keys fell back to their
//!   default or were ignored: the values load as before, but the loader names
//!   every such key in a `tracing::warn!` so a mismatched document cannot
//!   downgrade `auto_continue` or `max_concurrency` without a trace.
//!
//! The middle ground exists because the GUI and `arkd` share one data
//! directory: `deny_unknown_fields` would turn a shared file into a boot
//! failure, so tolerance stays and the report takes its place.

use crate::domain::{diagnose, ConfigDiagnostics, RuntimeConfig};
use std::path::{Path, PathBuf};

/// `data/config.json` under `data_dir`.
pub fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

/// A configuration document that was read, plus what the loader had to default
/// or ignore to accept it.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    config: RuntimeConfig,
    diagnostics: Option<ConfigDiagnostics>,
}

impl LoadedConfig {
    /// The configuration to run with.
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// What the document deviated from the schema on. `None` means the file did
    /// not exist, which is not a deviation.
    pub fn diagnostics(&self) -> Option<&ConfigDiagnostics> {
        self.diagnostics.as_ref()
    }

    /// Consume the report, returning the configuration.
    pub fn into_config(self) -> RuntimeConfig {
        self.config
    }
}

/// Load the config file, reporting — without failing — on keys that fell back.
///
/// A missing file yields defaults with `diagnostics() == None`; an unreadable or
/// unparseable file is an error.
pub fn load_report(path: &Path) -> Result<LoadedConfig, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedConfig {
                config: RuntimeConfig::default(),
                diagnostics: None,
            })
        }
        Err(err) => return Err(format!("cannot read {}: {err}", path.display())),
    };
    let invalid =
        |err: serde_json::Error| format!("{} is not valid config JSON: {err}", path.display());
    // Parsed to a `Value` first so the schema comparison sees the keys that
    // were actually written, not the ones `#[serde(default)]` invented.
    let document: serde_json::Value = serde_json::from_str(&raw).map_err(invalid)?;
    let diagnostics = diagnose(&document);
    let config: RuntimeConfig = serde_json::from_value(document).map_err(invalid)?;
    config.validate()?;
    Ok(LoadedConfig {
        config,
        diagnostics: Some(diagnostics),
    })
}

/// Load the config file, falling back to defaults when it does not exist yet.
///
/// Warns — once, naming the keys — when the document only partially matched the
/// schema. The returned configuration is exactly what `load_report` produced, so
/// the warning never changes behavior.
pub fn load_or_default(path: &Path) -> Result<RuntimeConfig, String> {
    let loaded = load_report(path)?;
    if let Some(diagnostics) = loaded.diagnostics() {
        if !diagnostics.is_clean() {
            tracing::warn!(
                config = %path.display(),
                "config.json does not fully match the runtime schema; \
                 affected keys kept their default or were ignored: {}",
                diagnostics.describe()
            );
        }
    }
    Ok(loaded.into_config())
}

/// Write the config atomically (temp file + rename), creating the directory.
pub fn save(path: &Path, config: &RuntimeConfig) -> Result<(), String> {
    config.validate()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(config)
        .map_err(|err| format!("cannot encode config: {err}"))?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, body).map_err(|err| format!("cannot write {}: {err}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|err| format!("cannot replace {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ark-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Collects formatted log lines so a test can assert on the real warning
    /// instead of only on the data the warning was built from.
    #[derive(Clone, Default)]
    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    impl CaptureWriter {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CaptureWriter {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Run `body` with a scoped subscriber so the capture never touches the
    /// process-global subscriber other tests rely on.
    fn capture_logs<T>(body: impl FnOnce() -> T) -> (T, String) {
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .finish();
        let value = tracing::subscriber::with_default(subscriber, body);
        (value, writer.text())
    }

    /// The shape the GUI writes: top-level `serve` / `models` / `runtime` keys
    /// and none of the runtime's own fields.
    const GUI_SHAPED: &str = r#"{
        "serve": {"port": 18801},
        "models": [],
        "runtime": {"logLevel": "debug"},
        "contextCompaction": {"enabled": true}
    }"#;

    #[test]
    fn missing_file_yields_defaults_without_a_warning() {
        let dir = temp_dir("missing");
        let path = config_path(&dir);
        let (config, logs) = capture_logs(|| load_or_default(&path).unwrap());
        assert_eq!(config, RuntimeConfig::default());
        assert!(logs.is_empty(), "a first run must stay silent: {logs}");
        assert!(load_report(&path).unwrap().diagnostics().is_none());
    }

    #[test]
    fn roundtrip_preserves_fields() {
        let dir = temp_dir("roundtrip");
        let path = config_path(&dir);
        let mut config = RuntimeConfig {
            log_level: "debug".into(),
            ..RuntimeConfig::default()
        };
        config.tuning.max_rounds = 7;
        save(&path, &config).unwrap();
        assert_eq!(load_or_default(&path).unwrap(), config);
    }

    #[test]
    fn corrupt_file_is_an_error_not_a_reset() {
        let dir = temp_dir("corrupt");
        let path = config_path(&dir);
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(load_or_default(&path).is_err());
    }

    /// Defect D2: a GUI-shaped document used to load as silent defaults.
    #[test]
    fn gui_shaped_document_warns_and_keeps_defaults() {
        let dir = temp_dir("gui-shaped");
        let path = config_path(&dir);
        std::fs::write(&path, GUI_SHAPED).unwrap();

        let (config, logs) = capture_logs(|| load_or_default(&path).unwrap());

        assert_eq!(config, RuntimeConfig::default());
        assert_eq!(config.subagents.max_concurrency, 2);
        assert!(!config.subagents.auto_continue);
        assert!(logs.contains("WARN"), "expected a warning line: {logs}");
        assert!(logs.contains("config.json does not fully match"), "{logs}");
        assert!(logs.contains("defaulted=[port, data_dir"), "{logs}");
        assert!(logs.contains("ignored=[contextCompaction"), "{logs}");
    }

    /// The report is the testable half of the same load, so the arkd startup
    /// path can assert on it without parsing log text.
    #[test]
    fn report_names_defaulted_and_ignored_keys() {
        let dir = temp_dir("report");
        let path = config_path(&dir);
        std::fs::write(&path, GUI_SHAPED).unwrap();

        let loaded = load_report(&path).unwrap();
        let diagnostics = loaded.diagnostics().expect("a present file is diagnosed");
        assert_eq!(diagnostics.missing(), crate::domain::CONFIG_KEYS);
        assert_eq!(
            diagnostics.unknown(),
            ["contextCompaction", "models", "runtime", "serve"]
        );
        assert_eq!(loaded.config(), &RuntimeConfig::default());
    }

    #[test]
    fn arkd_shaped_document_loads_without_a_warning() {
        let dir = temp_dir("arkd-shaped");
        let path = config_path(&dir);
        let mut expected = RuntimeConfig {
            log_level: "debug".into(),
            default_autonomy: "unattended".into(),
            ..RuntimeConfig::default()
        };
        expected.subagents.max_concurrency = 6;
        expected.subagents.auto_continue = true;
        save(&path, &expected).unwrap();

        let (config, logs) = capture_logs(|| load_or_default(&path).unwrap());

        assert_eq!(config, expected);
        assert!(logs.is_empty(), "a matching file must stay silent: {logs}");
    }

    /// A partial document is tolerated (GUI coexistence) but every key that was
    /// defaulted is named, including nested ones.
    ///
    /// `tuning` carries a struct-level `#[serde(default)]`, so an empty table is
    /// a real fallback that must be reported field by field.
    #[test]
    fn partial_document_names_nested_keys() {
        let dir = temp_dir("partial");
        let path = config_path(&dir);
        std::fs::write(&path, br#"{"port": 19001, "tuning": {"max_rounds": 9}}"#).unwrap();

        let (config, logs) = capture_logs(|| load_or_default(&path).unwrap());

        assert_eq!(config.port, 19001);
        assert_eq!(config.tuning.max_rounds, 9);
        assert_eq!(config.log_level, "info");
        assert!(logs.contains("tuning.max_tokens"), "{logs}");
        assert!(logs.contains("subagents"), "{logs}");
    }

    /// An incomplete `subagents` table stays a parse error: that struct has no
    /// struct-level default, so `serde` rejects it instead of inventing values.
    /// This is the "parse failure" branch, not a silent fallback.
    #[test]
    fn incomplete_nested_table_is_a_parse_error_not_a_silent_default() {
        let dir = temp_dir("incomplete-nested");
        let path = config_path(&dir);
        std::fs::write(&path, br#"{"subagents": {"enabled": true}}"#).unwrap();

        let err = load_or_default(&path).unwrap_err();
        assert!(err.contains("max_concurrency"), "{err}");
    }
}
