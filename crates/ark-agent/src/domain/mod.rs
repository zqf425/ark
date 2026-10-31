//! Data types shared by every ark-agent layer. No behaviour beyond small pure
//! helpers lives here.

pub mod action;
pub mod input;
pub mod limits;
pub mod state;
pub mod tool;

pub use action::{Action, DispatchPlan, StormNote, SyntheticResult, TurnOutcome};
pub use input::{ApprovalAsk, InputKind, Progress, TurnInput, UserInputAsk};
pub use limits::TurnLimits;
pub use state::TurnState;
pub use tool::{ToolCallRequest, ToolExecution};
