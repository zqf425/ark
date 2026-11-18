//! HTTP boundary: route table, auth middleware, SSE, graceful shutdown (§7.9).
//!
//! Everything here is transport. Domain decisions live in `ark-agent`,
//! `ark-config` and `ark-store`; the composition root injects one
//! [`state::AppState`] and the boundary never learns which provider is behind it.

#![deny(clippy::all)]

pub mod api;
pub mod gates;
pub mod service;
pub mod state;

pub use api::{error::ApiError, routes::router};
pub use gates::InteractiveGates;
pub use service::serve::{bind, AppHandle};
pub use service::{
    plan, schedule_auto_continuation, sync_auto_continue_policy, todo_marker, validate_todos,
    ImService, SchedulerService, TodoWriteTool, MAX_TODO_TITLE_CHARS,
};
pub use state::{AppState, SharedUsage, TurnEnd, TurnJob, TurnRunner, TurnSource};
