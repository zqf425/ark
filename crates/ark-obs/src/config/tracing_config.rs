//! Tracing configuration (design 7.11).
//!
//! Kept in `config` because the level, format and filter are settings, not
//! behaviour: the runtime reads them from `ark-config` and hands them here.

use tracing_subscriber::EnvFilter;

/// Default filter when the environment says nothing.
pub const DEFAULT_FILTER: &str = "info";

/// How log lines are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable single line, for a terminal or the GUI log pane.
    Compact,
    /// JSON lines, for shipping to a log collector.
    Json,
}

impl LogFormat {
    /// Parse a settings value; unknown values fall back to `Compact`.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "json" => LogFormat::Json,
            _ => LogFormat::Compact,
        }
    }
}

/// One tracing setup request.
#[derive(Debug, Clone)]
pub struct TracingConfig {
    filter: String,
    format: LogFormat,
    ansi: bool,
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            filter: DEFAULT_FILTER.to_string(),
            format: LogFormat::Compact,
            ansi: true,
        }
    }
}

impl TracingConfig {
    pub fn new(filter: impl Into<String>, format: LogFormat) -> Self {
        let filter = filter.into();
        Self {
            filter: if filter.trim().is_empty() {
                DEFAULT_FILTER.to_string()
            } else {
                filter
            },
            format,
            ansi: true,
        }
    }

    /// Read the filter from `ARK_LOG` when set (`RUST_LOG` is also accepted).
    pub fn from_env() -> Self {
        let filter = std::env::var("ARK_LOG")
            .or_else(|_| std::env::var("RUST_LOG"))
            .unwrap_or_else(|_| DEFAULT_FILTER.to_string());
        let format = LogFormat::parse(
            &std::env::var("ARK_LOG_FORMAT").unwrap_or_else(|_| "compact".to_string()),
        );
        Self::new(filter, format)
    }

    pub fn with_ansi(mut self, ansi: bool) -> Self {
        self.ansi = ansi;
        self
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn format(&self) -> LogFormat {
        self.format
    }

    pub fn ansi(&self) -> bool {
        self.ansi
    }

    /// Build the subscriber filter, falling back to the default on a bad spec.
    pub fn env_filter(&self) -> EnvFilter {
        EnvFilter::try_new(&self.filter).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_filters_fall_back_to_the_default() {
        assert_eq!(
            TracingConfig::new("   ", LogFormat::Json).filter(),
            DEFAULT_FILTER
        );
    }

    #[test]
    fn formats_parse_case_insensitively() {
        assert_eq!(LogFormat::parse("JSON"), LogFormat::Json);
        assert_eq!(LogFormat::parse("compact"), LogFormat::Compact);
        assert_eq!(LogFormat::parse("nonsense"), LogFormat::Compact);
    }

    #[test]
    fn invalid_filter_specs_do_not_panic() {
        let config = TracingConfig::new("this is not a filter = = =", LogFormat::Compact);
        // Constructing the filter must not panic; a bad spec degrades to default.
        let _ = config.env_filter();
    }
}
