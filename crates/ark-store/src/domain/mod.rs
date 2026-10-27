//! Pure domain rules: layout, projections, compaction markers, child index.
//!
//! Nothing here performs I/O, so every rule can be unit-tested without a
//! temporary directory.

pub mod child_index;
pub mod compaction;
pub mod diagnostics;
pub mod items;
pub mod layout;
pub mod retarget;
pub mod session_state;
pub mod thread_file;

pub use child_index::{
    fold_child_runs, index_from_runs, is_active_status, ChildIndexEntry, ChildIndexFile,
    ChildLocation, ChildLocator,
};
pub use compaction::{is_compacted, parse_marker, CompactionMarker};
pub use diagnostics::{DiagnosticKind, DiagnosticSink, MemoryDiagnostics, StoreDiagnostic};
pub use items::{fold_item_snapshots, keyed_item_id, project_items, ItemProjector};
pub use layout::{names, DataPaths, ThreadPaths};
pub use retarget::retarget_event;
pub use session_state::{SessionState, SessionStatus};
pub use thread_file::ThreadFile;
