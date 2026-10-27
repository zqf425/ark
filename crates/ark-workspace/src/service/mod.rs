//! Workspace services: the inspector and its hard-timeout git runner.
//!
//! `git_runner` is the only place a child process is spawned; `inspector`
//! composes the domain rules around it.

pub mod git_runner;
pub mod inspector;

pub use git_runner::{run_git, GitOutput, GIT_TIMEOUT, POLL_INTERVAL};
pub use inspector::WorkspaceInspector;
