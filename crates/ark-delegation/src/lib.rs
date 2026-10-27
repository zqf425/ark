//! ark-delegation: sub-agent runs (design §7.8).
//!
//! There is exactly **one** decision loop in this runtime: `ark-agent`'s reactor.
//! A child run is that same reactor executed against a child session, so this
//! crate never contains a second agent. What it owns is everything *around* a
//! child run that the reactor must not know about:
//!
//! * the concurrency budget and its wait queue ([`domain::SlotGate`]);
//! * the lifecycle and its persisted record (`children/runs.jsonl`);
//! * abort and steering handles for running children;
//! * the `<parent> -> children` activity events the parent UI subscribes to.
//!
//! # Why the executor is a port
//!
//! [`ChildTurnExecutor`] is implemented by `arkd`, because only the composition
//! root may wire a model client, a tool runner and a session writer together.
//! This crate therefore depends on `ark-ports` alone and stays testable with a
//! fake executor that never touches a provider.

#![deny(clippy::all)]

pub mod domain;
pub mod service;

pub use domain::{
    acquire_error, finished_record, ChildTurnContext, ChildTurnReport, ChildTurnStatus, SlotGate,
    SlotGuard, StopReason, ERR_QUEUE_FULL,
};
pub use service::{ChildTurnExecutor, DelegationRuntime, PreparedChild, RunningChild};
