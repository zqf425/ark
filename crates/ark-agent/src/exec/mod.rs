//! Boundary seams the reactor needs, implemented by `arkd`'s wiring /
//! `ark-store` / `ark-tool`. Nothing in this module performs I/O on its own
//! except the deliberately in-memory test doubles.
//!
//! The cancellation token and the steering queue used to live here; they moved
//! to `ark-ports` (re-exported at this crate's root) because the delegation
//! runtime must name the same concrete types without depending on `ark-agent`.

pub mod events;
pub mod gate;
pub mod journal;
pub mod tool_runner;

pub use events::TurnEvents;
pub use gate::{
    AllowAllGates, ApprovalReply, ApprovalRequest, DenyAllGates, GateBroker, GateFuture,
    UserInputRequest,
};
pub use journal::{EventSinkWriter, MemoryWriter, SessionWriter};
pub use tool_runner::{MapToolRunner, ToolRunner};
