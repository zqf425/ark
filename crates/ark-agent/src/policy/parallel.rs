//! Tool-batch planning: which calls may run concurrently (design §7.1 `policy/`).
//!
//! The rule is deliberately conservative. A call is only batched with another
//! when both are known read-only **and** they touch different resources.
//! Anything unknown is treated as a write and runs alone, so a new tool can
//! never accidentally race because nobody updated this file.

use std::collections::BTreeSet;

use crate::domain::tool::ToolCallRequest;

/// Coarse access class of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccessKind {
    /// Safe to run beside other reads of *different* resources.
    ReadOnly,
    /// Must run alone.
    Mutating,
}

/// Canonical identity of a call, used for repeat counting.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolSignature(pub String);

impl ToolSignature {
    pub fn of(call: &ToolCallRequest) -> Self {
        Self(call.signature())
    }
}

/// Stateless batch planner.
#[derive(Debug, Clone)]
pub struct ParallelPolicy {
    read_only: BTreeSet<String>,
}

impl Default for ParallelPolicy {
    /// Built-in read-only tools of the default catalogue. Unknown names are
    /// classified as mutating on purpose.
    fn default() -> Self {
        Self {
            read_only: [
                "read",
                "grep",
                "find",
                "ls",
                "glob",
                "web_fetch",
                "web_search",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        }
    }
}

impl ParallelPolicy {
    /// Replace the read-only set (e.g. from a plugin manifest).
    pub fn with_read_only<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            read_only: names.into_iter().map(Into::into).collect(),
        }
    }

    /// Add one tool to the read-only set.
    pub fn allow_parallel(mut self, tool: impl Into<String>) -> Self {
        self.read_only.insert(tool.into());
        self
    }

    pub fn classify(&self, call: &ToolCallRequest) -> AccessKind {
        if self.read_only.contains(&call.tool) {
            AccessKind::ReadOnly
        } else {
            AccessKind::Mutating
        }
    }

    /// Group calls into sequential batches. Concurrency is allowed **inside** a
    /// batch only; the reactor awaits batches in order.
    ///
    /// Batching is decided here and nowhere else, so the reactor can stay a
    /// dumb executor of the plan.
    pub fn plan(&self, calls: &[ToolCallRequest]) -> Vec<Vec<ToolCallRequest>> {
        let mut batches: Vec<Vec<ToolCallRequest>> = Vec::new();
        let mut pending: Vec<ToolCallRequest> = Vec::new();
        let mut pending_resources: BTreeSet<String> = BTreeSet::new();

        for call in calls {
            if self.classify(call) == AccessKind::Mutating {
                // A mutating call flushes the pending read batch and runs alone.
                flush(&mut batches, &mut pending, &mut pending_resources);
                batches.push(vec![call.clone()]);
                continue;
            }
            let resource = call.resource().unwrap_or_default().to_string();
            let conflicts = !resource.is_empty() && pending_resources.contains(&resource);
            if conflicts {
                flush(&mut batches, &mut pending, &mut pending_resources);
            }
            if !resource.is_empty() {
                pending_resources.insert(resource);
            }
            pending.push(call.clone());
        }
        flush(&mut batches, &mut pending, &mut pending_resources);
        batches
    }
}

fn flush(
    batches: &mut Vec<Vec<ToolCallRequest>>,
    pending: &mut Vec<ToolCallRequest>,
    pending_resources: &mut BTreeSet<String>,
) {
    if !pending.is_empty() {
        batches.push(std::mem::take(pending));
    }
    pending_resources.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, tool: &str, path: Option<&str>) -> ToolCallRequest {
        let arguments = match path {
            Some(path) => serde_json::json!({ "path": path }),
            None => serde_json::json!({}),
        };
        ToolCallRequest::new(id, tool, arguments)
    }

    fn ids(batch: &[ToolCallRequest]) -> Vec<&str> {
        batch.iter().map(|c| c.call_id.as_str()).collect()
    }

    #[test]
    fn unknown_tool_is_treated_as_mutating() {
        let policy = ParallelPolicy::default();
        assert_eq!(
            policy.classify(&call("c", "some_plugin_tool", None)),
            AccessKind::Mutating
        );
        assert_eq!(
            policy.classify(&call("c", "read", None)),
            AccessKind::ReadOnly
        );
    }

    #[test]
    fn reads_of_distinct_files_share_one_batch() {
        let policy = ParallelPolicy::default();
        let calls = vec![
            call("a", "read", Some("x.rs")),
            call("b", "read", Some("y.rs")),
        ];
        let batches = policy.plan(&calls);
        assert_eq!(batches.len(), 1);
        assert_eq!(ids(&batches[0]), vec!["a", "b"]);
    }

    #[test]
    fn reads_of_the_same_file_are_serialised() {
        let policy = ParallelPolicy::default();
        let calls = vec![
            call("a", "read", Some("x.rs")),
            call("b", "read", Some("x.rs")),
        ];
        let batches = policy.plan(&calls);
        assert_eq!(batches.len(), 2);
        assert_eq!(ids(&batches[0]), vec!["a"]);
        assert_eq!(ids(&batches[1]), vec!["b"]);
    }

    #[test]
    fn a_write_runs_alone_and_flushes_the_read_batch() {
        let policy = ParallelPolicy::default();
        let calls = vec![
            call("a", "read", Some("x.rs")),
            call("b", "write", Some("x.rs")),
            call("c", "read", Some("y.rs")),
        ];
        let batches = policy.plan(&calls);
        assert_eq!(batches.len(), 3);
        assert_eq!(ids(&batches[0]), vec!["a"]);
        assert_eq!(ids(&batches[1]), vec!["b"]);
        assert_eq!(ids(&batches[2]), vec!["c"]);
    }

    #[test]
    fn consecutive_writes_never_share_a_batch() {
        let policy = ParallelPolicy::default();
        let calls = vec![call("a", "bash", None), call("b", "bash", None)];
        let batches = policy.plan(&calls);
        assert_eq!(batches.len(), 2);
        assert!(batches.iter().all(|batch| batch.len() == 1));
    }

    #[test]
    fn empty_input_yields_no_batches() {
        assert!(ParallelPolicy::default().plan(&[]).is_empty());
    }

    #[test]
    fn custom_read_only_set_is_respected() {
        let policy = ParallelPolicy::with_read_only(["custom_read"]);
        assert_eq!(
            policy.classify(&call("c", "custom_read", None)),
            AccessKind::ReadOnly
        );
        assert_eq!(
            policy.classify(&call("c", "read", None)),
            AccessKind::Mutating
        );
        let extended = policy.allow_parallel("read");
        assert_eq!(
            extended.classify(&call("c", "read", None)),
            AccessKind::ReadOnly
        );
    }

    #[test]
    fn signature_is_stable_for_repeat_counting() {
        let a = call("a", "read", Some("x.rs"));
        let b = call("b", "read", Some("x.rs"));
        assert_eq!(ToolSignature::of(&a), ToolSignature::of(&b));
    }
}
