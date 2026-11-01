//! The durable cancel-reason vocabulary (design §7.1 "中断语义").
//!
//! A cancellation reason is written into event logs and read back after a
//! restart, so it is a wire-stable vocabulary, not a free string: three
//! process-owned reasons exist, and the boundary branches on them (whether a
//! turn was aborted by user intent decides suppression and user-facing copy).
//! Free strings here would make a typo a behavior change.

/// Why a turn was cancelled by the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// Ordinary user intent (an interrupt request, or aborting all live turns
    /// on behalf of one).
    UserInterrupt,
    /// The process is shutting down; no turn can outlive it.
    Shutdown,
    /// The owning process vanished and a later run converged the log.
    RuntimeRestart,
}

impl CancelReason {
    /// The durable string stored in the token and the event log.
    pub const fn as_durable_str(&self) -> &'static str {
        match self {
            CancelReason::UserInterrupt => "turn interrupted",
            CancelReason::Shutdown => "runtime shutting down",
            CancelReason::RuntimeRestart => "runtime restarted before turn could finish",
        }
    }

    /// Classify a durable string. `None` for anything the process did not
    /// write, so unknown historical reasons are never mistaken for a
    /// process-owned one.
    pub fn from_durable(reason: &str) -> Option<Self> {
        Self::all_durable()
            .iter()
            .find(|(_, durable)| *durable == reason)
            .map(|(reason, _)| *reason)
    }

    /// Every reason paired with its durable string, for classification and tests.
    pub fn all_durable() -> [(CancelReason, &'static str); 3] {
        [
            (CancelReason::UserInterrupt, "turn interrupted"),
            (CancelReason::Shutdown, "runtime shutting down"),
            (
                CancelReason::RuntimeRestart,
                "runtime restarted before turn could finish",
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_round_trips_every_reason() {
        for (reason, durable) in CancelReason::all_durable() {
            assert_eq!(CancelReason::from_durable(durable), Some(reason));
            assert_eq!(reason.as_durable_str(), durable);
        }
    }

    #[test]
    fn unknown_or_unknown_prefix_is_not_a_process_reason() {
        assert_eq!(
            CancelReason::from_durable("turn interrupted during tool dispatch"),
            None
        );
        assert_eq!(CancelReason::from_durable(""), None);
        assert_eq!(
            CancelReason::from_durable("user asked for something else"),
            None
        );
    }
}
