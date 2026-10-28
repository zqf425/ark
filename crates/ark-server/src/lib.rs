//! HTTP boundary: route table, auth middleware, SSE, graceful shutdown (§7.9).
//!
//! Everything here is transport. Domain decisions live in `ark-agent`,
//! `ark-config` and `ark-store`; the composition root injects one
//! [`state::AppState`] and the boundary never learns which provider is behind it.

pub mod api;
pub mod gates;
pub mod service;
pub mod state;

pub use api::{error::ApiError, routes::router};
pub use gates::InteractiveGates;
pub use service::serve::{bind, AppHandle};
pub use state::{AppState, SharedUsage, TurnEnd, TurnJob, TurnRunner};
