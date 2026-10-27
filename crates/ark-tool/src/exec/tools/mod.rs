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
pub mod search;
pub mod write;

pub use bash::BashTool;
pub use delegate::DelegateTaskTool;
pub use list::{FindTool, LsTool};
pub use read::ReadTool;
pub use search::GrepTool;
pub use write::{EditTool, WriteTool};

use std::sync::Arc;

use ark_ports::{SubagentRunner, Tool};

/// Every builtin, in canonical (alphabetical) order.
///
/// `delegate_task` is absent on purpose: it needs a runner. See
/// [`delegation_tools`].
pub fn builtins() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(BashTool),
        Box::new(EditTool),
        Box::new(FindTool),
        Box::new(GrepTool),
        Box::new(LsTool),
        Box::new(ReadTool),
        Box::new(WriteTool),
    ]
}

/// The delegation tool, for a runtime that has a sub-agent runner.
///
/// A child's tool set is built from [`builtins`] only, which is what enforces
/// `max_depth = 1`: a child has no `delegate_task` to call.
pub fn delegation_tools(runner: Arc<dyn SubagentRunner>) -> Vec<Box<dyn Tool>> {
    vec![Box::new(DelegateTaskTool::new(runner))]
}
