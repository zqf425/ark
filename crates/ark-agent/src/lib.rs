//! ark-agent: the single agent — explicit turn state machine (design §7.1).
//!
//! # Layout
//!
//! ```text
//! domain/   data only: states, inputs, actions, limits, tool calls
//! policy/   pure decisions: budget, tool-storm, parallel safety
//! context/  token estimation and history hygiene
//! service/  step (pure) + machine (invariants) + reactor (the only I/O)
//! exec/     cancellation token, journal, gates, tool runner seam
//! ```
//!
//! # Single-agent guarantee
//!
//! This crate contains exactly **one** decision loop: [`service::Reactor`]. No
//! module here calls `tokio::spawn` to start a second agent loop, and no module
//! hides a private `loop { call model … dispatch tools }` cycle. A sub-agent,
//! when one is added later, is another *turn* driven by the same reactor — never
//! a parallel loop with its own state. `service::reactor::tests` asserts that
//! the crate contains no detached loop and that the reactor is the only place
//! that awaits a model stream.
//!
//! # Purity
//!
//! [`step`] is a pure function `(TurnState, TurnInput) -> Action`: no I/O, no
//! clock, no randomness, no mutation. That is what makes every transition
//! unit-testable without HTTP or a mock provider. [`TurnMachine`] owns the
//! mutable turn state and the invariant assertions; the reactor turns an
//! [`Action`] into at most one boundary call.

#![deny(clippy::all)]

pub mod context;
pub mod domain;
pub mod exec;
pub mod policy;
pub mod service;

pub use context::{CompletionInbox, HistoryHygiene, HygieneReport, TokenEstimator};
pub use domain::{
    Action, ApprovalAsk, DispatchPlan, InputKind, Progress, StormNote, SyntheticResult,
    ToolCallRequest, ToolExecution, TurnInput, TurnLimits, TurnOutcome, TurnState,
};
pub use exec::{
    AllowAllGates, ApprovalReply, ApprovalRequest, CancelToken, DenyAllGates, EventSinkWriter,
    GateBroker, MemoryWriter, SessionWriter, SteerQueue, ToolRunner, TurnEvents, UserInputRequest,
};
pub use policy::{
    AccessKind, BudgetVerdict, ParallelPolicy, StormBreaker, StormReport, SuppressedCall,
    ToolSignature, TurnBudget,
};
pub use service::{run_calls, step, MachineError, ModelSeed, Reactor, ReactorConfig, TurnMachine};
