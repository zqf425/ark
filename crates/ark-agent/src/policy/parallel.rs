//! Tool-batch planning: which calls may run concurrently (design §7.1 `policy/`).
//!
//! The rule is deliberately conservative. A call is only batched with another
//! when both declare a non-exclusive [`ToolConcurrency`] **and** they do not
//! touch the same resource. Anything that does not declare otherwise is
//! `Exclusive` and runs alone, so a new tool can never accidentally race
//! because nobody updated this file.
//!
//! The class comes from the tool catalogue ([`ConcurrencyTable`]), which the
//! composition root reads from the registry once per turn. A hard-coded
//! allowlist of names lived here before; it could not express plugin tools and
//! misclassified every shell as a write, which serialised rounds that were
//! safe to run at once.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ark_ports::ToolConcurrency;

use crate::domain::tool::ToolCallRequest;

pub use ark_ports::ToolConcurrency as Concurrency;

/// Declared concurrency class per tool name.
///
/// A snapshot, not a live view: the registry hands one out when a turn is
/// built, so a mid-turn tool registration cannot change how an already-planned
/// round is batched. Cheap to clone and cheap to compare, so `TurnInput` can
/// carry it.
pub type ConcurrencyTable = Arc<BTreeMap<String, ToolConcurrency>>;

/// Build a table from `(name, concurrency)` pairs.
pub fn concurrency_table<I, S>(entries: I) -> ConcurrencyTable
where
    I: IntoIterator<Item = (S, ToolConcurrency)>,
    S: Into<String>,
{
    Arc::new(
        entries
            .into_iter()
            .map(|(name, class)| (name.into(), class))
            .collect(),
    )
}

/// The builtin catalogue's classes, used when no registry snapshot is wired.
///
/// Mirrors `ark-tool`'s declarations; it exists so `ark-agent`'s own tests and
/// any caller without a registry still batch sensibly. A name missing here is
/// `Exclusive`, which stays correct for plugin and MCP tools.
fn builtin_table() -> BTreeMap<String, ToolConcurrency> {
    [
        ("read", ToolConcurrency::ReadOnly),
        ("ls", ToolConcurrency::ReadOnly),
        ("find", ToolConcurrency::ReadOnly),
        ("grep", ToolConcurrency::ReadOnly),
        ("web_fetch", ToolConcurrency::ReadOnly),
        ("bash_output", ToolConcurrency::ReadOnly),
        // Every shell and child run owns a process, so two of them cannot
        // corrupt each other; serialising them only cost wall-clock time.
        ("bash", ToolConcurrency::Isolated),
        ("bash_session", ToolConcurrency::Isolated),
        ("bash_kill", ToolConcurrency::Isolated),
        ("delegate_task", ToolConcurrency::Isolated),
    ]
    .into_iter()
    .map(|(name, class)| (name.to_string(), class))
    .collect()
}

/// Coarse access class of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccessKind {
    /// Safe to run beside other reads of *different* resources.
    ReadOnly,
    /// Occupies its own process; safe beside anything touching other resources.
    Isolated,
    /// Must run alone.
    Mutating,
}

impl AccessKind {
    fn of(class: ToolConcurrency) -> Self {
        match class {
            ToolConcurrency::ReadOnly => Self::ReadOnly,
            ToolConcurrency::Isolated => Self::Isolated,
            ToolConcurrency::Exclusive => Self::Mutating,
        }
    }

    fn shares_a_batch(self) -> bool {
        self != Self::Mutating
    }
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
    table: ConcurrencyTable,
    /// Cap on calls executed concurrently inside one batch; `0` means no cap.
    /// A capped batch is split into sequential chunks, which keeps ordering
    /// while bounding fan-out.
    parallel_cap: usize,
}

impl Default for ParallelPolicy {
    /// Builtin classes only; unknown names are exclusive on purpose.
    fn default() -> Self {
        Self {
            table: Arc::new(builtin_table()),
            parallel_cap: 0,
        }
    }
}

impl ParallelPolicy {
    /// Plan against an explicit catalogue snapshot.
    pub fn with_table(table: ConcurrencyTable) -> Self {
        Self {
            table,
            parallel_cap: 0,
        }
    }

    /// Replace the concurrency catalogue.
    pub fn set_table(&mut self, table: ConcurrencyTable) {
        self.table = table;
    }

    /// Cap the number of calls that run concurrently in one batch.
    pub fn with_parallel_cap(mut self, cap: u32) -> Self {
        self.parallel_cap = cap as usize;
        self
    }

    pub fn classify(&self, call: &ToolCallRequest) -> AccessKind {
        self.classify_tool(&call.tool)
    }

    /// Class of a tool name; an unlisted tool is exclusive.
    pub fn classify_tool(&self, tool: &str) -> AccessKind {
        AccessKind::of(
            self.table
                .get(tool)
                .copied()
                .unwrap_or(ToolConcurrency::Exclusive),
        )
    }

    /// Group calls into sequential batches. Concurrency is allowed **inside** a
    /// batch only; the reactor awaits batches in order.
    ///
    /// Two calls join the same batch when neither is `Mutating` and their
    /// resources differ. A call with no resource at all (a shell, a delegation)
    /// conflicts with nothing, which is exactly why it needs the `Isolated`
    /// declaration to be eligible in the first place.
    pub fn plan(&self, calls: &[ToolCallRequest]) -> Vec<Vec<ToolCallRequest>> {
        let mut batches: Vec<Vec<ToolCallRequest>> = Vec::new();
        let mut pending: Vec<ToolCallRequest> = Vec::new();
        let mut pending_resources: BTreeSet<String> = BTreeSet::new();

        for call in calls {
            let kind = self.classify(call);
            if !kind.shares_a_batch() {
                // A mutating call flushes the pending batch and runs alone.
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
        self.cap(batches)
    }

    /// Split batches that exceed the concurrency cap into ordered chunks.
    fn cap(&self, batches: Vec<Vec<ToolCallRequest>>) -> Vec<Vec<ToolCallRequest>> {
        if self.parallel_cap == 0 {
            return batches;
        }
        batches
            .into_iter()
            .flat_map(|batch| {
                batch
                    .chunks(self.parallel_cap)
                    .map(<[ToolCallRequest]>::to_vec)
                    .collect::<Vec<_>>()
            })
            .collect()
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

    fn call(id: &str, tool: &str, args: serde_json::Value) -> ToolCallRequest {
        ToolCallRequest::new(id, tool, args)
    }

    fn read(id: &str, path: &str) -> ToolCallRequest {
        call(id, "read", serde_json::json!({ "path": path }))
    }

    fn bash(id: &str, command: &str) -> ToolCallRequest {
        call(id, "bash", serde_json::json!({ "command": command }))
    }

    fn ids(batch: &[ToolCallRequest]) -> Vec<&str> {
        batch.iter().map(|c| c.call_id.as_str()).collect()
    }

    fn sizes(batches: &[Vec<ToolCallRequest>]) -> Vec<usize> {
        batches.iter().map(Vec::len).collect()
    }

    fn exclusive(name: &str) -> ToolCallRequest {
        call("x", name, serde_json::json!({}))
    }

    #[test]
    fn unknown_tool_is_treated_as_mutating() {
        let policy = ParallelPolicy::default();
        assert_eq!(
            policy.classify(&exclusive("some_plugin_tool")),
            AccessKind::Mutating
        );
        assert_eq!(policy.classify(&exclusive("read")), AccessKind::ReadOnly);
    }

    #[test]
    fn shell_calls_share_one_batch() {
        // The measured regression: three `sleep 3` shells took 9s because each
        // one got its own batch. They own separate processes, so one batch is
        // the correct plan.
        let calls = vec![
            bash("a", "sleep 3"),
            bash("b", "sleep 3"),
            bash("c", "sleep 3"),
        ];
        let batches = ParallelPolicy::default().plan(&calls);
        assert_eq!(sizes(&batches), vec![3]);
        assert_eq!(ids(&batches[0]), vec!["a", "b", "c"]);
    }

    #[test]
    fn shell_and_reads_share_one_batch() {
        let calls = vec![read("a", "x.rs"), bash("b", "sleep 3"), read("c", "y.rs")];
        let batches = ParallelPolicy::default().plan(&calls);
        assert_eq!(sizes(&batches), vec![3]);
    }

    #[test]
    fn reads_of_distinct_files_share_one_batch() {
        let calls = vec![read("a", "x.rs"), read("b", "y.rs")];
        let batches = ParallelPolicy::default().plan(&calls);
        assert_eq!(sizes(&batches), vec![2]);
    }

    #[test]
    fn reads_of_the_same_file_are_serialised() {
        let calls = vec![read("a", "x.rs"), read("b", "x.rs")];
        let batches = ParallelPolicy::default().plan(&calls);
        assert_eq!(sizes(&batches), vec![1, 1]);
        assert_eq!(ids(&batches[0]), vec!["a"]);
        assert_eq!(ids(&batches[1]), vec!["b"]);
    }

    #[test]
    fn a_write_runs_alone_and_flushes_the_read_batch() {
        let calls = vec![
            read("a", "x.rs"),
            call("b", "write", serde_json::json!({ "path": "x.rs" })),
            read("c", "y.rs"),
        ];
        let batches = ParallelPolicy::default().plan(&calls);
        assert_eq!(sizes(&batches), vec![1, 1, 1]);
        assert_eq!(ids(&batches[0]), vec!["a"]);
        assert_eq!(ids(&batches[1]), vec!["b"]);
        assert_eq!(ids(&batches[2]), vec!["c"]);
    }

    #[test]
    fn consecutive_writes_never_share_a_batch() {
        let calls = vec![
            call("a", "write", serde_json::json!({ "path": "a" })),
            call("b", "write", serde_json::json!({ "path": "b" })),
        ];
        let batches = ParallelPolicy::default().plan(&calls);
        assert_eq!(sizes(&batches), vec![1, 1]);
    }

    #[test]
    fn empty_input_yields_no_batches() {
        assert!(ParallelPolicy::default().plan(&[]).is_empty());
    }

    #[test]
    fn parallel_cap_splits_large_batches() {
        let policy = ParallelPolicy::default().with_parallel_cap(2);
        let calls = vec![bash("a", "1"), bash("b", "2"), bash("c", "3")];
        let batches = policy.plan(&calls);
        assert_eq!(sizes(&batches), vec![2, 1]);
        assert_eq!(ids(&batches[0]), vec!["a", "b"]);
        assert_eq!(ids(&batches[1]), vec!["c"]);
    }

    #[test]
    fn zero_parallel_cap_means_unlimited() {
        let calls = (0..5)
            .map(|index| bash(&format!("c{index}"), "sleep 1"))
            .collect::<Vec<_>>();
        assert_eq!(ParallelPolicy::default().plan(&calls).len(), 1);
    }

    #[test]
    fn a_plugin_tool_can_declare_itself_isolated_without_core_changes() {
        // The old name allowlist could never reach this shape: a team ships a
        // tool, declares it isolated, and the core batches it beside others.
        let table = concurrency_table([
            ("read", ToolConcurrency::ReadOnly),
            ("team_plugin_shell", ToolConcurrency::Isolated),
        ]);
        let policy = ParallelPolicy::with_table(Arc::clone(&table));
        let calls = vec![
            read("a", "x.rs"),
            call("b", "team_plugin_shell", serde_json::json!({})),
        ];
        assert_eq!(sizes(&policy.plan(&calls)), vec![2]);
        assert_eq!(
            policy.classify(&exclusive("team_plugin_shell")),
            AccessKind::Isolated
        );
    }

    #[test]
    fn a_declared_exclusive_tool_stays_serialised() {
        let table = concurrency_table([("bash", ToolConcurrency::Exclusive)]);
        let policy = ParallelPolicy::with_table(table);
        let calls = vec![bash("a", "1"), bash("b", "2")];
        assert_eq!(sizes(&policy.plan(&calls)), vec![1, 1]);
    }

    #[test]
    fn a_name_missing_from_the_table_is_exclusive() {
        let policy =
            ParallelPolicy::with_table(concurrency_table([("read", ToolConcurrency::ReadOnly)]));
        assert_eq!(policy.classify(&exclusive("bash")), AccessKind::Mutating);
    }

    #[test]
    fn signatures_are_stable_for_repeat_counting() {
        let a = read("a", "x.rs");
        let b = read("b", "x.rs");
        assert_eq!(ToolSignature::of(&a), ToolSignature::of(&b));
    }
}
