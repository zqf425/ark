//! Installing the global tracing subscriber exactly once.
//!
//! `main` installs it; tests and plugins may try again. A second install must
//! return an error instead of panicking, otherwise a single test that enables
//! logging would abort the whole test binary.

use crate::config::{LogFormat, TracingConfig};
use std::fmt;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer;
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
    let filter: EnvFilter = config.env_filter();
    let registry = tracing_subscriber::registry().with(filter);

    match config.format() {
        LogFormat::Compact => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(config.ansi())
                    .with_target(true),
            )
            .try_init()
            .map_err(|err| err.to_string())?,
        LogFormat::Json => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .event_format(JsonLines::default()),
            )
            .try_init()
            .map_err(|err| err.to_string())?,
    }

    Ok(TracingGuard::default())
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
        mut writer: Writer<'_>,
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
    fn json_formatter_escapes_quotes() {
        let visitor = MessageVisitor {
            message: "say \"hi\"".to_string(),
        };
        let encoded = serde_json::to_string(&visitor.message).unwrap();
        assert_eq!(encoded, "\"say \\\"hi\\\"\"");
    }
}
