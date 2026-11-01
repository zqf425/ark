//! Tool registries assembled at startup: the parent set and the child set.

use std::sync::Arc;

use super::*;

/// Register every builtin, `web_fetch` and `delegate_task` for the parent agent.
///
/// A conflict here is a programming error.
pub(super) fn build_tools(
    runner: Arc<dyn ark_ports::SubagentRunner>,
    skills: Arc<ark_skills::SkillRuntime>,
    http: Arc<dyn ark_ports::HttpCapability>,
) -> Result<ToolRegistry> {
    let registry = build_child_tools(skills, http)?;
    registry.register_all(ark_tool::delegation_tools(runner))?;
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
    registry.register_all(ark_tool::builtins())?;
    registry.register_all(ark_tool::web_tools(http))?;
    registry.register(Arc::new(LoadSkillTool::new(skills)))?;
    Ok(registry)
}
