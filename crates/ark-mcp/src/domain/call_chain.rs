//! Call-chain guards for the in-process MCP bus (design 7.5.2).
//!
//! Three distinct failure modes are rejected *before* any handler runs:
//!
//! 1. **Depth** - a plugin chain deeper than [`MAX_DEPTH`] fan-outs into an
//!    unbounded recursion of model calls.
//! 2. **Cycle** - `A -> B -> A` on one task would never return; the chain is
//!    the stack, so an already-entered frame is a cycle.
//! 3. **Deadlock** - two *concurrent* tasks, `A` waiting for `B` while `B`
//!    waits for `A`. The chain is empty in each task, so this needs a shared
//!    wait-for graph instead.
//!
//! Everything here is pure and synchronous: the async plumbing lives in
//! `service::bus` and only calls these decisions.

use ark_common::{ArkError, ArkErrorCode};

/// Nested inter-plugin calls allowed by default, matching design 7.5.2.
pub const MAX_DEPTH: usize = 3;
/// Hard ceiling a manifest/grant may not exceed.
pub const MAX_CHAIN: usize = 16;

/// Why a call was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    /// `depth` frames are already active and `max` is the limit.
    DepthExceeded { depth: usize, max: usize },
    /// The requested tool is already on this task's stack.
    Cycle { trace: Vec<String> },
    /// Adding this wait would close a cycle in the wait-for graph.
    Deadlock { edge: (String, String) },
}

impl ChainError {
    /// Stable machine code, surfaced as the `detail` of an [`ArkError`].
    pub fn code(&self) -> &'static str {
        match self {
            ChainError::DepthExceeded { .. } => "mcp_depth_exceeded",
            ChainError::Cycle { .. } => "mcp_call_cycle",
            ChainError::Deadlock { .. } => "mcp_deadlock",
        }
    }

    pub fn message(&self) -> String {
        match self {
            ChainError::DepthExceeded { depth, max } => {
                format!("mcp call depth {depth} exceeds the limit of {max}")
            }
            ChainError::Cycle { trace } => {
                format!("mcp call cycle detected: {}", trace.join(" -> "))
            }
            ChainError::Deadlock { edge } => format!(
                "mcp deadlock: {} and {} are waiting for each other",
                edge.0, edge.1
            ),
        }
    }

    /// Denials are the caller's error, never an internal failure.
    pub fn into_error(self) -> ArkError {
        let code = self.code();
        ArkError::new(ArkErrorCode::ValidationError, self.message()).with_detail(code)
    }
}

/// One tool invocation in a chain, rendered as `<plugin>.<tool>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CallFrame {
    pub plugin_id: String,
    pub tool: String,
}

impl CallFrame {
    pub fn new(plugin_id: impl Into<String>, tool: impl Into<String>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            tool: tool.into(),
        }
    }

    pub fn key(&self) -> String {
        format!("{}.{}", self.plugin_id, self.tool)
    }
}

/// The stack of frames active on **one** task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    frames: Vec<CallFrame>,
    max_depth: usize,
}

impl Default for Chain {
    fn default() -> Self {
        Self::with_limit(MAX_DEPTH)
    }
}

impl Chain {
    /// `max_depth` is clamped to `[1, MAX_CHAIN]`: a grant cannot widen the
    /// limit beyond the hard ceiling.
    pub fn with_limit(max_depth: usize) -> Self {
        Self {
            frames: Vec::new(),
            max_depth: max_depth.clamp(1, MAX_CHAIN),
        }
    }

    pub fn max_depth(&self) -> usize {
        self.max_depth
    }

    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    pub fn frames(&self) -> &[CallFrame] {
        &self.frames
    }

    /// `["a.x", "b.y"]` - the form that appears in an error and in the trace.
    pub fn trace(&self) -> Vec<String> {
        self.frames.iter().map(CallFrame::key).collect()
    }

    /// Push a frame, or refuse the call. Depth is checked before the cycle so a
    /// too-deep chain reports the actionable reason first.
    pub fn enter(&mut self, provider: &str, tool: &str) -> Result<(), ChainError> {
        let frame = CallFrame::new(provider, tool);
        if self.frames.len() >= self.max_depth {
            return Err(ChainError::DepthExceeded {
                depth: self.frames.len() + 1,
                max: self.max_depth,
            });
        }
        if self.frames.contains(&frame) {
            let mut trace = self.trace();
            trace.push(frame.key());
            return Err(ChainError::Cycle { trace });
        }
        self.frames.push(frame);
        Ok(())
    }

    /// Pop the innermost frame. Called on every exit path, including errors.
    pub fn exit(&mut self) {
        self.frames.pop();
    }

    /// A fresh chain with no frames (a new ingress call).
    pub fn root(max_depth: usize) -> Self {
        Self::with_limit(max_depth)
    }

    /// Container of the keys currently on the stack, for cheap membership tests.
    pub fn contains(&self, provider: &str, tool: &str) -> bool {
        let frame = CallFrame::new(provider, tool);
        self.frames.contains(&frame)
    }
}

/// Wait-for graph across concurrent calls.
///
/// Node = plugin id. An edge `A -> B` means "a call issued by `A` is currently
/// blocked on an in-process call served by `B`". Registering an edge that
/// closes a cycle is a deadlock and is refused.
#[derive(Debug, Default, Clone)]
pub struct DeadlockGuard {
    edges: Vec<(String, String)>,
}

impl DeadlockGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register "waiter is blocked on target". Returns `Deadlock` when the pair
    /// would close a cycle (including `A -> A`).
    pub fn register_wait(&mut self, waiter: &str, target: &str) -> Result<(), ChainError> {
        let edge = (waiter.to_string(), target.to_string());
        let closes_cycle = waiter == target || self.reaches(target, waiter);
        if closes_cycle {
            return Err(ChainError::Deadlock { edge });
        }
        if !self.edges.contains(&edge) {
            self.edges.push(edge);
        }
        Ok(())
    }

    /// Drop the edge once the awaited call returned.
    pub fn release(&mut self, waiter: &str, target: &str) {
        self.edges
            .retain(|(from, to)| !(from == waiter && to == target));
    }

    /// Is there a path `from -> ... -> to`?
    pub fn reaches(&self, from: &str, to: &str) -> bool {
        let mut seen: Vec<&str> = vec![from];
        let mut queue: Vec<&str> = vec![from];
        while let Some(current) = queue.pop() {
            for (edge_from, edge_to) in &self.edges {
                if edge_from != current {
                    continue;
                }
                if edge_to == to {
                    return true;
                }
                if !seen.contains(&edge_to.as_str()) {
                    seen.push(edge_to);
                    queue.push(edge_to);
                }
            }
        }
        false
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }
}

/// Test helper: reset process-wide state. Kept as a no-op entry point so tests
/// can document that the guards themselves hold no globals.
pub fn reset_for_tests() {}

/// RAII frame used by the bus: `Drop` pops even when the handler panics or the
/// future is cancelled.
pub struct CallGuard<'a> {
    chain: &'a mut Chain,
    active: bool,
}

impl<'a> CallGuard<'a> {
    pub fn new(chain: &'a mut Chain, provider: &str, tool: &str) -> Result<Self, ChainError> {
        chain.enter(provider, tool)?;
        Ok(Self {
            chain,
            active: true,
        })
    }

    pub fn depth(&self) -> usize {
        self.chain.depth()
    }

    /// Consume the guard, popping the frame exactly once.
    pub fn finish(mut self) {
        self.pop();
    }

    fn pop(&mut self) {
        if self.active {
            self.chain.exit();
            self.active = false;
        }
    }
}

impl Drop for CallGuard<'_> {
    fn drop(&mut self) {
        self.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_nested_frames_are_allowed_and_the_fourth_is_not() {
        let mut chain = Chain::root(MAX_DEPTH);
        assert!(chain.enter("ark.a", "one").is_ok());
        assert!(chain.enter("ark.b", "two").is_ok());
        assert!(chain.enter("ark.c", "three").is_ok());
        let err = chain.enter("ark.d", "four").unwrap_err();
        assert_eq!(
            err,
            ChainError::DepthExceeded {
                depth: 4,
                max: MAX_DEPTH
            }
        );
        assert_eq!(err.code(), "mcp_depth_exceeded");
    }

    #[test]
    fn depth_limit_cannot_exceed_the_hard_ceiling() {
        let chain = Chain::with_limit(1_000);
        assert_eq!(chain.max_depth(), MAX_CHAIN);
        let chain = Chain::with_limit(0);
        assert_eq!(chain.max_depth(), 1);
    }

    #[test]
    fn a_b_a_is_reported_as_a_cycle() {
        let mut chain = Chain::root(MAX_CHAIN);
        chain.enter("ark.a", "run").unwrap();
        chain.enter("ark.b", "call").unwrap();
        let err = chain.enter("ark.a", "run").unwrap_err();
        assert_eq!(err.code(), "mcp_call_cycle");
        assert!(
            err.message()
                .contains("ark.a.run -> ark.b.call -> ark.a.run"),
            "{}",
            err.message()
        );
    }

    #[test]
    fn the_same_plugin_may_reenter_a_different_tool() {
        let mut chain = Chain::root(MAX_DEPTH);
        chain.enter("ark.a", "one").unwrap();
        assert!(chain.enter("ark.a", "two").is_ok());
    }

    #[test]
    fn exit_pops_and_frees_the_depth_budget() {
        let mut chain = Chain::root(2);
        chain.enter("ark.a", "one").unwrap();
        chain.enter("ark.b", "two").unwrap();
        chain.exit();
        assert_eq!(chain.depth(), 1);
        assert!(chain.enter("ark.c", "three").is_ok());
    }

    #[test]
    fn call_guard_pops_on_drop() {
        let mut chain = Chain::root(MAX_DEPTH);
        {
            let guard = CallGuard::new(&mut chain, "ark.a", "run").unwrap();
            assert_eq!(guard.depth(), 1);
        }
        assert_eq!(chain.depth(), 0);
        assert!(
            chain.enter("ark.a", "run").is_ok(),
            "frame must be reusable after drop"
        );
    }

    #[test]
    fn call_guard_finish_pops_once() {
        let mut chain = Chain::root(MAX_DEPTH);
        let guard = CallGuard::new(&mut chain, "ark.a", "run").unwrap();
        guard.finish();
        assert_eq!(chain.depth(), 0);
    }

    #[test]
    fn mutual_waiting_is_reported_as_a_deadlock() {
        let mut guard = DeadlockGuard::new();
        assert!(guard.register_wait("ark.a", "ark.b").is_ok());
        let err = guard.register_wait("ark.b", "ark.a").unwrap_err();
        assert_eq!(err.code(), "mcp_deadlock");
        assert!(err.message().contains("ark.a") && err.message().contains("ark.b"));
        // The refused edge must not be recorded.
        assert_eq!(guard.edge_count(), 1);
    }

    #[test]
    fn a_longer_wait_cycle_is_also_a_deadlock() {
        let mut guard = DeadlockGuard::new();
        guard.register_wait("a", "b").unwrap();
        guard.register_wait("b", "c").unwrap();
        let err = guard.register_wait("c", "a").unwrap_err();
        assert_eq!(err.code(), "mcp_deadlock");
    }

    #[test]
    fn waiting_on_yourself_is_a_deadlock() {
        let mut guard = DeadlockGuard::new();
        assert_eq!(
            guard.register_wait("ark.a", "ark.a").unwrap_err().code(),
            "mcp_deadlock"
        );
    }

    #[test]
    fn release_breaks_the_cycle_and_allows_a_retry() {
        let mut guard = DeadlockGuard::new();
        guard.register_wait("ark.a", "ark.b").unwrap();
        assert!(guard.register_wait("ark.b", "ark.a").is_err());
        guard.release("ark.a", "ark.b");
        assert!(guard.is_empty());
        assert!(guard.register_wait("ark.b", "ark.a").is_ok());
    }

    #[test]
    fn independent_waiters_do_not_conflict() {
        let mut guard = DeadlockGuard::new();
        guard.register_wait("ark.a", "ark.shared").unwrap();
        guard.register_wait("ark.b", "ark.shared").unwrap();
        assert_eq!(guard.edge_count(), 2);
    }

    #[test]
    fn chain_error_maps_to_a_validation_error_with_a_stable_code() {
        let err = ChainError::Deadlock {
            edge: ("ark.a".into(), "ark.b".into()),
        }
        .into_error();
        assert_eq!(err.detail.as_deref(), Some("mcp_deadlock"));
        assert_eq!(err.code, ArkErrorCode::ValidationError);
    }
}
