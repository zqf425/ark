//! One module per route group, mirroring the table in `routes.rs`.
//!
//! Handlers stay thin: parse the request, call a port, map the error. Business
//! rules live behind the ports, which is what keeps the HTTP layer testable with
//! fakes and prevents a second copy of a domain rule here.

pub mod attachments;
pub mod base;
pub mod child_runs;
pub mod config;
pub mod config_view;
pub mod diagnostics;
pub mod events;
pub mod gates;
pub mod health;
pub mod memory;
pub mod pending;
pub mod rewind;
pub mod runtime;
pub mod subagents;
pub mod threads;
pub mod turns;
pub mod workbench;
