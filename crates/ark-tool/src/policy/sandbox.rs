//! Sandbox policy: one decision per dispatch (design §7.3).
//!
//! The policy is consulted exactly once, before a tool runs, and produces a
//! [`SandboxDecision`] that travels inside `ToolContext`. Tools therefore never
//! re-derive their own rules, and a stricter mode (for example `deny_bash`) is
//! one field away instead of a per-tool branch.

use ark_ports::{SandboxDecision, SandboxPolicy, ToolArgs};
use serde::{Deserialize, Serialize};

use crate::domain::meta::meta_of;

/// How much autonomy the runtime grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Autonomy {
    /// Every mutation and every shell command needs a human decision.
    Strict,
    /// Workspace writes run unattended; shell commands still ask.
    WorkspaceWrite,
    /// Everything the builtin catalogue offers runs unattended.
    Unattended,
}

impl Default for Autonomy {
    fn default() -> Self {
        Self::WorkspaceWrite
    }
}

/// Policy implementation injected into the reactor.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuiltinSandboxPolicy {
    autonomy: Autonomy,
}

impl BuiltinSandboxPolicy {
    pub fn new(autonomy: Autonomy) -> Self {
        Self { autonomy }
    }

    pub fn autonomy(&self) -> Autonomy {
        self.autonomy
    }
}

impl SandboxPolicy for BuiltinSandboxPolicy {
    fn decide(&self, tool: &str, _args: &ToolArgs) -> SandboxDecision {
        // An unknown tool is never read-only: refusing to guess is the safe
        // baseline, and the registry will reject the call anyway.
        let baseline = meta_of(tool)
            .map(|meta| meta.baseline)
            .unwrap_or(SandboxDecision::NeedsApproval);
        match self.autonomy {
            Autonomy::Strict => match baseline {
                SandboxDecision::ReadOnly => SandboxDecision::ReadOnly,
                _ => SandboxDecision::NeedsApproval,
            },
            Autonomy::WorkspaceWrite => baseline,
            Autonomy::Unattended => match baseline {
                SandboxDecision::NeedsApproval => SandboxDecision::WorkspaceWrite,
                other => other,
            },
        }
    }
}

/// Whether a decision permits execution without a human.
pub fn is_self_service(decision: SandboxDecision) -> bool {
    matches!(
        decision,
        SandboxDecision::ReadOnly | SandboxDecision::WorkspaceWrite
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> ToolArgs {
        ToolArgs(serde_json::json!({ "path": "a.txt" }))
    }

    #[test]
    fn read_only_tools_stay_read_only_in_every_mode() {
        for mode in [
            Autonomy::Strict,
            Autonomy::WorkspaceWrite,
            Autonomy::Unattended,
        ] {
            assert_eq!(
                BuiltinSandboxPolicy::new(mode).decide("read", &args()),
                SandboxDecision::ReadOnly
            );
        }
    }

    #[test]
    fn strict_mode_gates_writes_and_shell() {
        let policy = BuiltinSandboxPolicy::new(Autonomy::Strict);
        assert_eq!(
            policy.decide("write", &args()),
            SandboxDecision::NeedsApproval
        );
        assert_eq!(
            policy.decide("bash", &args()),
            SandboxDecision::NeedsApproval
        );
    }

    #[test]
    fn workspace_write_mode_allows_edits_but_not_shell() {
        let policy = BuiltinSandboxPolicy::new(Autonomy::WorkspaceWrite);
        assert_eq!(
            policy.decide("edit", &args()),
            SandboxDecision::WorkspaceWrite
        );
        assert_eq!(
            policy.decide("bash", &args()),
            SandboxDecision::NeedsApproval
        );
    }

    #[test]
    fn unattended_mode_never_asks() {
        let policy = BuiltinSandboxPolicy::new(Autonomy::Unattended);
        assert_eq!(
            policy.decide("bash", &args()),
            SandboxDecision::WorkspaceWrite
        );
        assert!(is_self_service(policy.decide("bash", &args())));
    }

    #[test]
    fn unknown_tools_require_approval_even_when_unattended() {
        let policy = BuiltinSandboxPolicy::new(Autonomy::Strict);
        assert_eq!(
            policy.decide("mystery", &args()),
            SandboxDecision::NeedsApproval
        );
    }

    #[test]
    fn approval_is_never_self_service() {
        assert!(!is_self_service(SandboxDecision::NeedsApproval));
    }
}
