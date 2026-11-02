//! Child-run lifecycle vocabulary (design §7.8).
//!
//! One enum for the five states a child run records, shared by the durable
//! log (`children/runs.jsonl`), the wire DTO and the ports. It replaces the
//! former string-constants module: with no stored history to stay compatible
//! with, an unknown spelling is a corrupt log and should fail loudly instead
//! of degrading to a magic string.

use crate::error::{ArkError, Result};
use serde::{Deserialize, Serialize};

/// Lifecycle of one child run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "ChildRunStatus.ts")]
#[serde(rename_all = "snake_case")]
pub enum ChildRunStatus {
    /// Accepted, waiting for a concurrency slot.
    #[default]
    Queued,
    /// A slot is held and the turn is running.
    Running,
    /// Terminal: the child answered.
    Completed,
    /// Terminal: the child errored.
    Failed,
    /// Terminal: the child was aborted.
    Aborted,
}

impl ChildRunStatus {
    /// Every state, in lifecycle order.
    pub const ALL: [ChildRunStatus; 5] = [
        ChildRunStatus::Queued,
        ChildRunStatus::Running,
        ChildRunStatus::Completed,
        ChildRunStatus::Failed,
        ChildRunStatus::Aborted,
    ];

    /// The durable string stored in the run log.
    pub const fn as_str(&self) -> &'static str {
        match self {
            ChildRunStatus::Queued => "queued",
            ChildRunStatus::Running => "running",
            ChildRunStatus::Completed => "completed",
            ChildRunStatus::Failed => "failed",
            ChildRunStatus::Aborted => "aborted",
        }
    }

    /// Parse a log string. Unknown values are corrupt records, not data.
    pub fn parse(value: &str) -> Result<Self> {
        Self::ALL
            .iter()
            .find(|status| status.as_str() == value)
            .copied()
            .ok_or_else(|| ArkError::validation(format!("unknown child run status: {value}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_every_state() {
        for status in ChildRunStatus::ALL {
            assert_eq!(
                ChildRunStatus::parse(status.as_str()).expect("valid state"),
                status
            );
        }
        assert!(ChildRunStatus::parse("bogus").is_err());
    }

    #[test]
    fn serde_uses_the_durable_strings() {
        for status in ChildRunStatus::ALL {
            let json = serde_json::to_string(&status).expect("serializable");
            assert_eq!(json, format!("\"{}\"", status.as_str()));
        }
    }
}
