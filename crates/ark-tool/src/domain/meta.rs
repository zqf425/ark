//! Static tool metadata (design §7.3, appendix C).
//!
//! The table is data, not a `match` in three places: the registry, the sandbox
//! policy and `GET /v1/runtime/tools` all read the same rows.

use ark_ports::SandboxDecision;

/// One builtin tool's static facts.
#[derive(Debug, Clone, Copy)]
pub struct ToolMeta {
    /// Wire name, e.g. `read`.
    pub name: &'static str,
    /// One-line description used in the JSON schema.
    pub description: &'static str,
    /// Decision applied when the policy has no stricter opinion.
    pub baseline: SandboxDecision,
}

/// The builtin catalogue, in canonical (alphabetical) order.
///
/// Order is part of the contract: the schema fingerprint and the prompt prefix
/// must be byte-identical between turns, so this array is the single source of
/// the ordering.
///
/// `delegate_task` is listed here even though it is not part of
/// [`crate::exec::tools::builtins`], because the sandbox policy and
/// `GET /v1/runtime/tools` read this table: an unlisted tool would silently be
/// treated as "unknown, so needs approval" and delegation would always ask.
pub const TOOL_METAS: [ToolMeta; 8] = [
    ToolMeta {
        name: "bash",
        description: "Run one shell command inside the workspace with a hard timeout",
        baseline: SandboxDecision::NeedsApproval,
    },
    ToolMeta {
        name: "delegate_task",
        description: "Hand one bounded sub-task to a child run and read its final answer",
        // Delegation itself writes nothing: the child's own tools carry their
        // own sandbox decisions, and the runtime enforces the concurrency budget.
        baseline: SandboxDecision::ReadOnly,
    },
    ToolMeta {
        name: "edit",
        description: "Replace an exact string in one file",
        baseline: SandboxDecision::WorkspaceWrite,
    },
    ToolMeta {
        name: "find",
        description: "Find files whose name matches a glob below a directory",
        baseline: SandboxDecision::ReadOnly,
    },
    ToolMeta {
        name: "grep",
        description: "Search file contents for a pattern",
        baseline: SandboxDecision::ReadOnly,
    },
    ToolMeta {
        name: "ls",
        description: "List one directory level",
        baseline: SandboxDecision::ReadOnly,
    },
    ToolMeta {
        name: "read",
        description: "Read a slice of one file",
        baseline: SandboxDecision::ReadOnly,
    },
    ToolMeta {
        name: "write",
        description: "Write a whole file inside the workspace",
        baseline: SandboxDecision::WorkspaceWrite,
    },
];

/// Metadata for `name`, if it is a builtin.
pub fn meta_of(name: &str) -> Option<&'static ToolMeta> {
    TOOL_METAS.iter().find(|meta| meta.name == name)
}

/// Every name in canonical order.
pub fn names() -> Vec<String> {
    TOOL_METAS
        .iter()
        .map(|meta| meta.name.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_sorted_for_a_stable_prefix() {
        let listed = names();
        let mut sorted = listed.clone();
        sorted.sort();
        assert_eq!(listed, sorted, "canonical order must be alphabetical");
    }

    #[test]
    fn every_builtin_has_metadata() {
        for name in names() {
            assert!(meta_of(&name).is_some());
        }
        assert!(meta_of("nope").is_none());
    }

    #[test]
    fn read_tools_stay_read_only() {
        for name in ["read", "ls", "find", "grep"] {
            assert_eq!(meta_of(name).unwrap().baseline, SandboxDecision::ReadOnly);
        }
    }
}
