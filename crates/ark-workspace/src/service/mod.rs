//! Workspace services: the inspector and its hard-timeout git runner.
//!
//! `git_runner` is the only place a child process is spawned; `inspector`
//! composes the domain rules around it. The runner stays crate-private: raw
//! process output is parsed behind the stable `WorkspaceInspector` contract and
//! must not become a public escape hatch.

pub(crate) mod git_runner;
pub mod inspector;
pub mod project_instructions;

pub use inspector::WorkspaceInspector;
pub use project_instructions::seed_for_workspace as project_instructions_seed;
