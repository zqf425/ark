//! Scheduled-task persistence boundary.
//!
//! One small document (`<data_dir>/schedules.json`) holds every task and its
//! bounded run history, so the port is a plain document CRUD surface rather
//! than an appendable log: a schedule fires at most once a minute per task, and
//! "what is the next run" is a field on the task, not a derived scan.
//!
//! Sync like [`crate::AttachmentStore`], not `async` like
//! [`crate::SessionStore`]: the whole document is read or rewritten under one
//! in-process lock, so there is nothing to await and no benefit in handing the
//! critical section to the executor.
//!
//! ## Where policy lives
//!
//! The store assigns an id when the caller leaves one empty, persists exactly
//! what it is given, and never normalizes. Cadence repair, the 100-task cap and
//! `next_run_at` all live in `ark-schedule`, because a store that silently
//! rewrote a task would make the stored document differ from the response the
//! client just received. The one ordering rule the caller must honour is
//! therefore explicit: read `list`, run the cap check, then `insert` — the
//! window between them is acceptable for a single-user desktop runtime, and
//! `ark-schedule::ensure_capacity` is what turns it into a structured
//! `schedule_limit` error.

use ark_common::Result;
use ark_contract::ScheduledTask;

/// Durable task table, owned by `ark-store` and injected by the composition root.
pub trait ScheduleStore: Send + Sync {
    /// Every stored task, most recently updated first.
    ///
    /// Ordering is the store's job because the file order is insertion order,
    /// while the settings page reads as "what changed lately".
    fn list(&self) -> Result<Vec<ScheduledTask>>;

    /// One task by id.
    ///
    /// Missing ids are `not_found`, never an empty default: a caller that
    /// silently received a placeholder task could schedule a run nobody asked
    /// for.
    fn get(&self, id: &str) -> Result<ScheduledTask>;

    /// Append a task, assigning `id` when the caller left it empty.
    ///
    /// Returns the stored task, which is how the caller learns the generated
    /// id. A duplicate id is a `conflict` rather than an overwrite, because
    /// `insert` is used for creates only and a silent overwrite would drop a
    /// task's history.
    fn insert(&self, task: ScheduledTask) -> Result<ScheduledTask>;

    /// Overwrite an existing task by id. An absent id is `not_found`.
    ///
    /// Mutable bookkeeping and edits prefer [`ScheduleStore::update`] so they
    /// cannot race another writer with a stale document.
    fn replace(&self, task: ScheduledTask) -> Result<ScheduledTask>;

    /// Read, mutate and persist one task while holding the store's write lock.
    ///
    /// Scheduler bookkeeping and HTTP edits use this instead of a stale
    /// `get`/`replace` pair. The candidate is cloned first, so a failed write
    /// leaves the in-memory document unchanged.
    fn update(
        &self,
        id: &str,
        mutate: &mut dyn FnMut(&mut ScheduledTask) -> Result<()>,
    ) -> Result<ScheduledTask>;

    /// Remove one task and its history.
    ///
    /// An absent id is `not_found`, so `DELETE /v1/schedules/{id}` can answer
    /// 404 instead of pretending it deleted something.
    fn delete(&self, id: &str) -> Result<()>;
}
