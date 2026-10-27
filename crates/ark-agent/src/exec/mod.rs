//! Boundary seams the reactor needs, owned by this crate (not by `ark-ports`).
//!
//! Everything here is either a cancellation primitive or a trait implemented by
//! `arkd`'s wiring / `ark-store` / `ark-tool`. Nothing in this module performs
//! I/O on its own except the deliberately in-memory test doubles.

pub mod cancel;
pub mod events;
pub mod gate;
pub mod journal;
pub mod steer;
pub mod tool_runner;

pub use cancel::CancelToken;
pub use events::TurnEvents;
pub use gate::{
    AllowAllGates, ApprovalReply, ApprovalRequest, DenyAllGates, GateBroker, UserInputRequest,
};
pub use journal::{EventSinkWriter, MemoryWriter, SessionWriter};
pub use steer::SteerQueue;
pub use tool_runner::{MapToolRunner, ToolRunner};
