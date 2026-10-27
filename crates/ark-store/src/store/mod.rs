//! Storage layer: files, logs, single-writer actors and the rebuildable index.
//!
//! Everything here performs I/O. Domain rules live in `crate::domain` so the
//! reconciliation and projection rules stay unit-testable without a disk.

pub mod child_store;
pub mod event_log;
pub mod fs_util;
pub mod index;
pub mod items_log;
pub mod notifier;
pub mod writer_actor;
pub mod writer_state;

#[cfg(test)]
pub(crate) mod tests;

pub use child_store::{
    active_child_runs_blocking, append_child_run_blocking, begin_children_delete_blocking,
    child_dir_ready_blocking, child_entries_blocking, finish_children_delete_blocking,
    is_deleting_dir, orphan_child_dirs_blocking, pending_tombstones_blocking,
    read_child_index_blocking, read_child_runs_blocking, rebuild_child_index_blocking,
    sync_child_index_blocking,
};
pub use event_log::{
    append_text, encode_event, encode_line, parse_log, plan_compaction, read_log_blocking,
    read_markers_blocking, repair_log_blocking, LogParse,
};
pub use fs_util::{
    append_line_blocking, ensure_dir, file_len, list_dirs, read_json_opt, remove_dir_all,
    rename_dir, write_atomic, write_json_atomic,
};
pub use index::{
    load_snapshot, plan_reconcile, save_snapshot, DiskFacts, IndexEntry, IndexProjector,
    IndexSnapshot, ReconcileAction, ThreadIndex,
};
pub use items_log::{
    append_item_blocking, append_usage_blocking, read_item_snapshots_blocking, read_items_blocking,
    read_usage_blocking, rebuild_items_blocking, thread_usage_blocking, usage_totals,
    write_items_blocking, UsageRecord,
};
pub use notifier::{IndexNotifier, NoopNotifier};
pub use writer_actor::{WriterCmd, WriterHandle};
pub use writer_state::{Snapshot, WriterState};
