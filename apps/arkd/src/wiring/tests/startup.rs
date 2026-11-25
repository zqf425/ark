use super::*;

/// Collects formatted log lines so the startup test can assert on the real
/// warning instead of only on the data behind it.
#[derive(Clone, Default)]
struct CaptureWriter(Arc<std::sync::Mutex<Vec<u8>>>);

impl CaptureWriter {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("capture lock")).into_owned()
    }
}

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("capture lock").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Run `body` under a scoped subscriber: the process-global subscriber is
/// left alone, so parallel tests never race on tracing installation.
fn capture_startup_logs<T>(body: impl FnOnce() -> T) -> (T, String) {
    let writer = CaptureWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_ansi(false)
        .finish();
    let value = tracing::subscriber::with_default(subscriber, body);
    (value, writer.text())
}

/// Defect D2: the GUI writes `serve`/`models`/`runtime`, so a copied file
/// used to start the runtime on pure defaults without a single log line.
#[test]
fn gui_shaped_config_warns_at_startup_and_keeps_defaults() {
    let data_dir = TestDataDir::new();
    std::fs::write(
        ark_config::config_path(data_dir.path()),
        br#"{"serve":{"port":18801},"models":[],"runtime":{"logLevel":"debug"}}"#,
    )
    .expect("config is writable");

    let (loaded, logs) = capture_startup_logs(|| load_startup_config(data_dir.path()));
    let loaded = loaded.expect("a parseable document still loads");

    assert_eq!(loaded.subagents.max_concurrency, 2);
    assert!(loaded.subagents.auto_continue);
    assert!(logs.contains("WARN"), "startup must warn: {logs}");
    assert!(logs.contains("defaulted=[port"), "{logs}");
    assert!(logs.contains("ignored=[models"), "{logs}");
}

#[test]
fn matching_config_starts_without_a_warning() {
    let data_dir = TestDataDir::new();
    let path = ark_config::config_path(data_dir.path());
    let mut config = RuntimeConfig::default();
    config.subagents.max_concurrency = 5;
    config.subagents.auto_continue = true;
    ark_config::save(&path, &config).expect("config is writable");

    let (loaded, logs) = capture_startup_logs(|| load_startup_config(data_dir.path()));

    assert_eq!(loaded.expect("config loads"), config);
    assert!(logs.is_empty(), "a matching file must stay silent: {logs}");
}

#[test]
fn missing_config_is_a_silent_first_run() {
    let data_dir = TestDataDir::new();
    let (loaded, logs) = capture_startup_logs(|| load_startup_config(data_dir.path()));
    assert_eq!(loaded.expect("defaults load"), RuntimeConfig::default());
    assert!(logs.is_empty(), "a first run must stay silent: {logs}");
}

#[test]
fn unparseable_config_still_fails_startup() {
    let data_dir = TestDataDir::new();
    std::fs::write(ark_config::config_path(data_dir.path()), b"{ not json")
        .expect("config is writable");
    assert!(load_startup_config(data_dir.path()).is_err());
}
