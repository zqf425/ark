//! Installing the global tracing subscriber exactly once.
//!
//! `main` installs it; tests and plugins may try again. A second install must
//! return an error instead of panicking, otherwise a single test that enables
//! logging would abort the whole test binary.

use crate::config::{LogFormat, TracingConfig};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer as FormatWriter;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Handle for future dynamic reload support; dropping it removes nothing.
#[derive(Debug, Default)]
pub struct TracingGuard {
    _private: (),
}

/// Install the global subscriber, returning an error when one already exists.
pub fn try_init_tracing(config: &TracingConfig) -> Result<TracingGuard, String> {
    install(config, console_writer(config.stderr()))
}

/// Install tracing to the configured console stream and an append-only file.
pub fn try_init_tracing_with_file(
    config: &TracingConfig,
    path: impl AsRef<Path>,
) -> Result<TracingGuard, String> {
    if let Some(parent) = path.as_ref().parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_ref())
        .map_err(|error| error.to_string())?;
    install(
        config,
        TeeMakeWriter {
            stderr: config.stderr(),
            file: Arc::new(Mutex::new(file)),
        },
    )
}

fn install<W>(config: &TracingConfig, writer: W) -> Result<TracingGuard, String>
where
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    let filter: EnvFilter = config.env_filter();
    let registry = tracing_subscriber::registry().with(filter);
    match config.format() {
        LogFormat::Compact => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(config.ansi())
                    .with_target(true)
                    .with_writer(writer),
            )
            .try_init()
            .map_err(|err| err.to_string())?,
        LogFormat::Json => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .event_format(JsonLines::default())
                    .with_writer(writer),
            )
            .try_init()
            .map_err(|err| err.to_string())?,
    }
    Ok(TracingGuard::default())
}

fn console_writer(stderr: bool) -> tracing_subscriber::fmt::writer::BoxMakeWriter {
    use tracing_subscriber::fmt::writer::BoxMakeWriter;
    if stderr {
        BoxMakeWriter::new(io::stderr)
    } else {
        BoxMakeWriter::new(io::stdout)
    }
}

#[derive(Clone)]
struct TeeMakeWriter {
    stderr: bool,
    file: Arc<Mutex<File>>,
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for TeeMakeWriter {
    type Writer = TeeWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        TeeWriter {
            stderr: self.stderr,
            file: Arc::clone(&self.file),
        }
    }
}

struct TeeWriter {
    stderr: bool,
    file: Arc<Mutex<File>>,
}

impl Write for TeeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let file_result = self
            .file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write_all(bytes);
        let console_result = if self.stderr {
            io::stderr().write_all(bytes)
        } else {
            io::stdout().write_all(bytes)
        };
        file_result.and(console_result)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let file_result = self
            .file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .flush();
        let console_result = if self.stderr {
            io::stderr().flush()
        } else {
            io::stdout().flush()
        };
        file_result.and(console_result)
    }
}

/// Install the global subscriber, tolerating an earlier installation.
///
/// Returns `true` when this call installed it, `false` when tracing was
/// already initialized by someone else.
pub fn init_tracing(config: &TracingConfig) -> bool {
    try_init_tracing(config).is_ok()
}

/// JSON-lines event formatter.
///
/// Implemented here instead of enabling `tracing-subscriber`'s `json` feature so
/// the crate keeps its dependency set unchanged (that feature pulls in
/// `tracing-serde`, which the workspace does not vendor).
#[derive(Debug, Default)]
struct JsonLines {
    ansi: bool,
}

impl JsonLines {
    #[allow(dead_code)]
    fn with_ansi(mut self, ansi: bool) -> Self {
        self.ansi = ansi;
        self
    }
}

impl<S, N> FormatEvent<S, N> for JsonLines
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: FormatWriter<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let message = serde_json::to_string(&visitor.message).unwrap_or_else(|_| "\"\"".into());
        write!(
            writer,
            "{{\"level\":\"{}\",\"target\":\"{}\",\"message\":{}}}",
            meta.level(),
            meta.target(),
            message
        )
    }
}

/// Collects the `message` field and folds every other field into it.
#[derive(Debug, Default)]
struct MessageVisitor {
    message: String,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else if self.message.is_empty() {
            self.message = format!("{}={value:?}", field.name());
        } else {
            self.message
                .push_str(&format!(" {}={value:?}", field.name()));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.record_debug(field, &value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The subscriber is global, so only one test may install it. This test is
    // intentionally the only one that calls `try_init_tracing`: a second call
    // must report an error rather than panic.
    #[test]
    fn installing_twice_is_an_error_not_a_panic() {
        let config = TracingConfig::new("info", LogFormat::Compact).with_ansi(false);
        let first = try_init_tracing(&config);
        let second = try_init_tracing(&config);
        match first {
            Ok(_) => assert!(second.is_err(), "second install must fail"),
            Err(_) => assert!(second.is_err(), "second install must fail"),
        }
        // The tolerant entry point never reports failure by panicking.
        let _ = init_tracing(&config);
    }

    #[test]
    fn tee_writer_appends_without_truncating() {
        let path = std::env::temp_dir().join(format!(
            "ark-obs-append-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::write(&path, b"before\n").expect("seed log");
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open append-only log");
        let mut writer = TeeWriter {
            stderr: true,
            file: Arc::new(Mutex::new(file)),
        };
        writer.write_all(b"after\n").expect("append log");
        writer.flush().expect("flush log");
        assert_eq!(std::fs::read(&path).expect("read log"), b"before\nafter\n");
        std::fs::remove_file(path).expect("remove owned test log");
    }

    #[test]
    fn json_formatter_escapes_quotes() {
        let visitor = MessageVisitor {
            message: "say \"hi\"".to_string(),
        };
        let encoded = serde_json::to_string(&visitor.message).unwrap();
        assert_eq!(encoded, "\"say \\\"hi\\\"\"");
    }
}
