//! Builtin tool implementations (design §7.3, appendix C).
//!
//! Seven zero-sized tools, one file per behaviour group, plus `delegate_task`.
//! Each implements [`ark_ports::Tool`]; none of them decides its own sandbox
//! rules, because the decision already travelled in `ToolContext`.
//!
//! `delegate_task` is the one exception to the zero-sized rule: it holds an
//! `Arc<dyn SubagentRunner>` so it can ask the delegation runtime for a child run
//! without `ark-tool` depending on that crate. It is therefore **not** part of
//! [`builtins`] and is added explicitly by the composition root.

pub mod bash;
pub mod delegate;
pub mod list;
pub mod read;
pub mod request_user_input;
pub mod search;
pub mod shell_session;
pub mod web_fetch;
pub mod write;

pub use bash::BashTool;
pub use delegate::DelegateTaskTool;
pub use list::{FindTool, LsTool};
pub use read::ReadTool;
pub use request_user_input::RequestUserInputTool;
pub use search::GrepTool;
pub use shell_session::{BashKillTool, BashOutputTool, BashSessionTool, ShellSessionManager};
pub use web_fetch::WebFetchTool;
pub use write::{EditTool, WriteTool};

use std::sync::Arc;

use ark_ports::{HttpCapability, SubagentPolicySource, SubagentRunner, Tool};

/// Every builtin, in canonical (alphabetical) order.
///
/// `delegate_task` is absent on purpose: it needs a runner. See
/// [`delegation_tools`]. `web_fetch` is likewise absent: it needs the governed
/// egress. See [`web_tools`].
pub fn builtins() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(BashTool),
        Box::new(EditTool),
        Box::new(FindTool),
        Box::new(GrepTool),
        Box::new(LsTool),
        Box::new(ReadTool),
        Box::new(RequestUserInputTool),
        Box::new(WriteTool),
    ]
}

/// Builtins plus the persistent shell session tools, sharing one manager.
///
/// `bash_session`/`bash_output`/`bash_kill` need a shared manager instance so
/// a background task started in one call stays pollable and killable from
/// later calls (requirements/shell-sessions.md). Runtimes that build the
/// plain [`builtins`] set keep today's behaviour; this constructor is what the
/// composition root uses when it wants session continuity.
pub fn builtins_with_shell_session() -> Vec<Box<dyn Tool>> {
    let session = Arc::new(ShellSessionManager::new());
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(BashSessionTool::new(Arc::clone(&session))),
        Box::new(BashOutputTool::new(Arc::clone(&session))),
        Box::new(BashKillTool::new(session)),
    ];
    tools.extend(builtins());
    tools
}

/// The delegation tool, for a runtime that has a sub-agent runner.
///
/// A child's tool set is built from [`builtins`] only, which is what enforces
/// `max_depth = 1`: a child has no `delegate_task` to call. When the runtime
/// also supplies a [`SubagentPolicySource`] the tool applies the live
/// `default_background` decision; pass `None` to keep the foreground default.
pub fn delegation_tools(
    runner: Arc<dyn SubagentRunner>,
    policy: Option<Arc<dyn SubagentPolicySource>>,
) -> Vec<Box<dyn Tool>> {
    let tool = DelegateTaskTool::new(runner);
    let tool = match policy {
        Some(policy) => tool.with_policy(policy),
        None => tool,
    };
    vec![Box::new(tool)]
}

/// The web fetch tool, for a runtime that has the governed egress.
///
/// Both the parent and the child sets get it: children legitimately need to
/// look up documentation, and the egress policy is the same for both.
pub fn web_tools(http: Arc<dyn HttpCapability>) -> Vec<Box<dyn Tool>> {
    vec![Box::new(WebFetchTool::new(http))]
}
