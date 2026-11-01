//! Shared policy vocabulary (design §5.2).
//!
//! `Autonomy` is one vocabulary consumed by four layers — `ark-config`
//! (settings validation), `ark-schedule` (task normalization), `ark-tool`
//! (sandbox policy) and `arkd` (child-run escalation) — so the enum, its wire
//! strings and its escalation order live here, next to the other shared
//! primitives. A string match in each consumer would let a new level be added
//! in one place and silently ignored in the other three.

use ark_common::{ArkError, Result};
use serde::{Deserialize, Serialize};

/// How much autonomy the runtime grants.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Autonomy {
    /// Every mutation and every shell command needs a human decision.
    Strict,
    /// Workspace writes run unattended; shell commands still ask.
    #[default]
    WorkspaceWrite,
    /// Everything the builtin catalogue offers runs unattended.
    Unattended,
}

impl Autonomy {
    /// Every level, in escalation order (least to most permissive).
    pub const ALL: [Autonomy; 3] = [
        Autonomy::Strict,
        Autonomy::WorkspaceWrite,
        Autonomy::Unattended,
    ];

    /// The wire string for this level (`serde` renames the same way).
    pub fn as_str(&self) -> &'static str {
        match self {
            Autonomy::Strict => "strict",
            Autonomy::WorkspaceWrite => "workspace_write",
            Autonomy::Unattended => "unattended",
        }
    }

    /// Parse a wire string. Unknown values are rejected rather than silently
    /// downgraded to the permissive default.
    pub fn parse(value: &str) -> Result<Self> {
        Self::ALL
            .iter()
            .find(|level| level.as_str() == value)
            .copied()
            .ok_or_else(|| {
                ArkError::validation(format!(
                    "invalid autonomy: {value}; expected one of {}",
                    Self::ALL
                        .iter()
                        .map(|l| l.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }

    /// Escalation rank: a child run may narrow but never widen its parent.
    pub fn rank(&self) -> u8 {
        match self {
            Autonomy::Strict => 0,
            Autonomy::WorkspaceWrite => 1,
            Autonomy::Unattended => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_every_wire_string_and_rejects_unknowns() {
        for level in Autonomy::ALL {
            assert_eq!(
                Autonomy::parse(level.as_str()).expect("valid wire string"),
                level
            );
        }
        assert!(
            Autonomy::parse("unattended ").is_err(),
            "no silent trimming"
        );
        assert!(Autonomy::parse("Unattended").is_err(), "case sensitive");
        assert!(Autonomy::parse("").is_err());
    }

    #[test]
    fn as_str_round_trips_serde() {
        for level in Autonomy::ALL {
            let json = serde_json::to_string(&level).expect("serializable");
            assert_eq!(json, format!("\"{}\"", level.as_str()));
        }
    }

    #[test]
    fn rank_orders_escalation() {
        assert!(Autonomy::Strict.rank() < Autonomy::WorkspaceWrite.rank());
        assert!(Autonomy::WorkspaceWrite.rank() < Autonomy::Unattended.rank());
    }
}
