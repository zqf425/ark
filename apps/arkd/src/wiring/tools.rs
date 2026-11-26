//! Tool registries assembled at startup: the parent set and the child set.

use std::sync::Arc;

use super::*;

/// Register every builtin, `web_fetch`, `delegate_task` and `todo_write` for
/// the parent agent.
///
/// A conflict here is a programming error.
pub(super) fn build_tools(
    runner: Arc<dyn ark_ports::SubagentRunner>,
    policy: Arc<dyn ark_ports::SubagentPolicySource>,
    skills: Arc<ark_skills::SkillRuntime>,
    http: Arc<dyn ark_ports::HttpCapability>,
    store: Arc<dyn SessionStore>,
) -> Result<ToolRegistry> {
    let registry = build_child_tools(skills, http)?;
    registry.register_all(ark_tool::delegation_tools(runner, Some(policy)))?;
    // The durable plan tool is parent-only: a child writes into its own child
    // thread's event log, which the parent plan projection never reads, so a
    // registration there would silently lose every write.
    registry.register(Arc::new(ark_server::TodoWriteTool::new(store)))?;
    Ok(registry)
}

/// The child tool set: builtins plus `web_fetch`.
///
/// `delegate_task` is absent on purpose. A child therefore has no way to spawn a
/// grandchild, which enforces `max_depth = 1` without trusting a runtime check
/// that a future refactor could bypass. `web_fetch` is present: children
/// legitimately need to look up documentation, and the egress policy is the
/// same for both sets.
pub(super) fn build_child_tools(
    skills: Arc<ark_skills::SkillRuntime>,
    http: Arc<dyn ark_ports::HttpCapability>,
) -> Result<ToolRegistry> {
    let registry = ToolRegistry::new();
    // The persistent shell session tools ride both sets (requirements/
    // shell-sessions.md): a child legitimately runs long commands and polls
    // them too. Each registry gets its own manager instance, so parent and
    // child tasks never share state.
    registry.register_all(ark_tool::builtins_with_shell_session())?;
    registry.register_all(ark_tool::web_tools(http))?;
    registry.register(Arc::new(LoadSkillTool::new(skills)))?;
    Ok(registry)
}

/// Merge the external-MCP and plugin tools into the child registry.
///
/// A delegated research task runs entirely inside the child, so a child whose
/// `web_fetch` fails has no way to fall back to a browser tool — the fallback
/// lives in the same MCP group the parent sees. Delegated work runs under the
/// parent's autonomy and the same egress governance, so exposing the group does
/// not widen what a child may *do*; it only stops the child from being the one
/// agent that cannot do it. Callers must keep this **after**
/// `build_child_tools` and before the first child turn: the merge is one
/// `replace_group`, so an MCP reconnect cannot evict plugin tools here either.
///
/// `delegate_task` stays absent: the group never contains it (children cannot
/// spawn grandchildren), and a qualified `mcp` group name cannot collide with a
/// builtin name anyway (`replace_group` still refuses one, by design).
pub(super) fn merge_child_mcp_tools(
    child_tools: &ToolRegistry,
    mcp_tools: Vec<Arc<dyn Tool>>,
) -> Result<()> {
    child_tools.replace_group("mcp", mcp_tools)
}
