//! Pure policies consulted by [`crate::service::step`].
//!
//! Each policy is a separate module so a rule change has exactly one home:
//! budgets in `budget`, the repeated-call breaker in `storm`, batching in
//! `parallel`. None of them performs I/O or reads shared mutable state.

pub mod budget;
pub mod parallel;
pub mod storm;

pub use budget::{BudgetVerdict, TurnBudget};
pub use parallel::{AccessKind, ParallelPolicy, ToolSignature};
pub use storm::{suppression_message, StormBreaker, StormReport, SuppressedCall};
