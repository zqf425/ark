//! ark-schedule: the pure scheduling domain for unattended tasks.
//!
//! The crate owns three things and nothing else: the task/cadence shapes (which
//! it re-exports from `ark-contract` so the wire, the persisted document and the
//! generated TypeScript cannot drift), the normalization rules that every
//! create/update path must pass through, and the `next_run_at` decision point.
//!
//! It performs no I/O and reads no clock: "now" is a parameter on every
//! function, which is what makes the full cadence matrix testable without
//! sleeping. Persistence goes through [`ark_ports::ScheduleStore`] and execution
//! is orchestrated by `ark-server`; neither is referenced here.

#![deny(clippy::all)]

pub mod history;
pub mod next_run;
pub mod types;

pub use history::{bound_history, push_record, record_run};
pub use next_run::{
    apply_next_run_at, compute_next_run_at, format_next_run_at, is_due, parse_stored_next_run,
};
pub use types::{
    apply_update, ensure_capacity, normalize_autonomy, normalize_hhmm, normalize_schedule,
    normalize_task, normalize_title, parse_hhmm, prepare_create, AUTONOMY_VALUES,
};
