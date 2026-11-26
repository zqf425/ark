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
use std::sync::{Mutex, MutexGuard};

/// Serializes replacements of the same process's config document. Unique temp
/// names isolate writers, while this lock avoids Windows sharing violations
/// between simultaneous `MoveFileExW` calls.
static SAVE_LOCK: Mutex<()> = Mutex::new(());

fn save_lock() -> MutexGuard<'static, ()> {
    SAVE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
    let _save_guard = save_lock();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(config)
        .map_err(|err| format!("cannot encode config: {err}"))?;
    let temp = temp_path(path);
    let result = write_temp(&temp, body.as_bytes()).and_then(|()| replace_file(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|err| format!("cannot replace {}: {err}", path.display()))
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");
    path.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()))
}

fn write_temp(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    file.write_all(body)?;
    file.sync_all()
}

#[cfg(windows)]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let wide =
        |value: &Path| -> Vec<u16> { value.as_os_str().encode_wide().chain(Some(0)).collect() };
    let from = wide(temp);
    let to = wide(path);
    // SAFETY: both NUL-terminated buffers live for the synchronous call.
    let moved = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = parent_directory(path);
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("cannot sync parent directory {}: {error}", parent.display()),
            )
        })
}

#[cfg(unix)]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    std::fs::rename(temp, path)?;
    // The file itself was synced before rename. Sync the directory afterwards so
    // the new name survives a crash; failure is reported even though rename has
    // already committed and must not be presented as an unchanged old document.
    sync_parent_directory(path)
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
    fn save_replaces_an_existing_document_on_windows() {
        let dir = temp_dir("replace");
        let path = config_path(&dir);
        let first = RuntimeConfig {
            log_level: "debug".into(),
            ..RuntimeConfig::default()
        };
        save(&path, &first).unwrap();
        let mut second = first.clone();
        second.log_level = "warn".into();
        save(&path, &second).unwrap();
        assert_eq!(load_or_default(&path).unwrap(), second);
    }

    #[cfg(unix)]
    #[test]
    fn unix_replace_syncs_the_parent_directory() {
        let dir = temp_dir("unix-replace");
        let path = config_path(&dir);
        let temp = temp_path(&path);
        std::fs::write(&temp, b"{}").expect("write temp");

        replace_file(&temp, &path).expect("rename and parent sync");

        assert!(!temp.exists());
        assert_eq!(std::fs::read(&path).expect("read replaced config"), b"{}");
        let error = sync_parent_directory(&dir.join("missing/config.json")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(error.to_string().contains("cannot sync parent directory"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_empty_parent_means_the_current_directory() {
        assert_eq!(parent_directory(Path::new("config.json")), Path::new("."));
    }

    #[test]
    fn concurrent_saves_use_independent_temp_files() {
        let dir = temp_dir("concurrent");
        let path = Arc::new(config_path(&dir));
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|index| {
                let path = Arc::clone(&path);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut config = RuntimeConfig::default();
                    barrier.wait();
                    for round in 0..20 {
                        config.tuning.max_rounds = index * 100 + round + 1;
                        save(&path, &config).expect("concurrent save succeeds");
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("save thread");
        }
        let stored = load_or_default(&path).expect("final document is complete");
        let round = stored.tuning.max_rounds % 100;
        let writer = stored.tuning.max_rounds / 100;
        assert!((1..=20).contains(&round), "unexpected round: {round}");
        assert!(writer <= 7, "unexpected writer: {writer}");
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
        assert!(config.subagents.auto_continue);
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
        // `hooks`, `skills` and `approvals` absences are deliberately silent
        // (additive optional sections).
        let expected_missing: Vec<&str> = crate::domain::CONFIG_KEYS
            .iter()
            .copied()
            .filter(|key| *key != "hooks" && *key != "skills" && *key != "approvals")
            .collect();
        assert_eq!(diagnostics.missing(), expected_missing);
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
