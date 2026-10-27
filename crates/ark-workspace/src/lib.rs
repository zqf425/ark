//! ark-workspace: Workspace inspection: path validation and git status (design 7.9).
//!
//! Layer folders follow the QDC mapping in design 5.1:
//!
//! ```text
//! domain/   data only: home expansion, path resolution, git output parsing
//! service/  WorkspaceInspector plus the hard-timeout git runner
//! ```
//!
//! Two invariants make `GET /v1/workspace/status` safe to call from the request
//! path while the user is typing:
//!
//! 1. every `git` invocation has a hard 3 s wall-clock limit, because a wedged
//!    git on Windows would otherwise stall the whole HTTP request;
//! 2. a missing directory short-circuits before any process is spawned.
//!
//! The wire shape is [`ark_contract::WorkspaceStatus`], which mirrors the
//! TypeScript contract field for field, so the renderer keeps working unchanged.

#![deny(clippy::all)]

pub mod domain;
pub mod service;

pub use domain::{
    base_status, clean_branch, clean_sha, count_status_lines, empty_status, expand_home, home_dir,
    resolve_candidate,
};
pub use service::{run_git, GitOutput, WorkspaceInspector, GIT_TIMEOUT, POLL_INTERVAL};
