//! Delegation depth policy (design 7.4.1).
//!
//! v1 allows exactly one level (parent -> child). The layout could nest further,
//! but the executor has always stripped `delegate_task` from a child's tool set,
//! so allowing depth 2 would advertise a capability the runtime does not have.
//! The limit therefore lives in code, with a message that explains the reason.

use ark_common::{ArkError, ChildId, ThreadId};

/// Maximum nesting depth for v1: parent -> child only.
pub const MAX_DEPTH: u8 = 1;

/// Depth policy carried by the store so callers cannot bypass it.
#[derive(Debug, Clone, Copy)]
pub struct DelegationPolicy {
    max_depth: u8,
}

impl Default for DelegationPolicy {
    fn default() -> Self {
        Self {
            max_depth: MAX_DEPTH,
        }
    }
}

/// Why a delegation request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegateError {
    /// The parent is already a child run, so nesting would exceed `MAX_DEPTH`.
    TooDeep,
}

impl DelegateError {
    pub fn into_ark_error(self, parent: ThreadId, parent_is_child: bool) -> ArkError {
        match self {
            DelegateError::TooDeep => {
                // The message must state the rule and the reason, not just "denied".
                let detail = if parent_is_child {
                    format!("{parent} is itself a child run")
                } else {
                    format!("{parent} would be at depth 2")
                };
                ArkError::validation(format!(
                    "delegation depth limit is {MAX_DEPTH} (parent -> child only): \
                     nested delegation is not supported by the executor in v1"
                ))
                .with_detail(detail)
            }
        }
    }
}

impl DelegationPolicy {
    pub fn new(max_depth: u8) -> Self {
        Self {
            max_depth: max_depth.min(MAX_DEPTH),
        }
    }

    pub fn max_depth(&self) -> u8 {
        self.max_depth
    }

    /// Check that `parent` may spawn a child at `parent_depth`.
    ///
    /// `parent_depth` is 0 for a top-level thread and 1 for a child run.
    pub fn check(
        &self,
        parent: ThreadId,
        parent_depth: u8,
    ) -> Result<(), (ThreadId, DelegateError)> {
        if parent_depth + 1 > self.max_depth {
            return Err((parent, DelegateError::TooDeep));
        }
        Ok(())
    }

    /// Whether the child directory is reachable for a given parent depth.
    pub fn child_dir(&self, parent: ThreadId, child: ChildId) -> Option<String> {
        if self.max_depth == 0 {
            return None;
        }
        Some(format!("{parent}/children/{child}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_parents_may_delegate_one_level() {
        let policy = DelegationPolicy::default();
        assert_eq!(policy.max_depth(), 1);
        assert!(policy.check(ThreadId::new(), 0).is_ok());
    }

    #[test]
    fn child_runs_cannot_delegate_again() {
        let policy = DelegationPolicy::default();
        let parent = ThreadId::new();
        let err = policy.check(parent, 1).unwrap_err();
        assert_eq!(err.1, DelegateError::TooDeep);
        let message = err.1.into_ark_error(err.0, true).message;
        assert!(message.contains("depth limit is 1"), "{message}");
        assert!(message.contains("nested delegation"), "{message}");
    }

    #[test]
    fn configured_depth_cannot_exceed_the_executor_limit() {
        assert_eq!(DelegationPolicy::new(5).max_depth(), MAX_DEPTH);
        assert_eq!(DelegationPolicy::new(0).max_depth(), 0);
    }

    #[test]
    fn child_dir_is_relative_to_the_parent() {
        let policy = DelegationPolicy::default();
        let parent = ThreadId::new();
        let child = ChildId::new();
        assert_eq!(
            policy.child_dir(parent, child),
            Some(format!("{parent}/children/{child}"))
        );
        assert!(DelegationPolicy::new(0).child_dir(parent, child).is_none());
    }
}
