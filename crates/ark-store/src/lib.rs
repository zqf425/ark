//! ark-store: session directory as system of record plus a rebuildable index.
//!
//! Design 7.4. The crate is layered so each rule has one home:
//!
//! * [`domain`] - pure rules (layout, event->item projection, compaction
//!   markers, child index folds, reconciliation planning). No I/O, so every
//!   rule is unit-testable.
//! * [`store`] - I/O: JSONL logs, atomic writes, the single-writer actor, the
//!   index snapshot, child-run records.
//! * [`service`] - the `FileSessionStore` facade that implements
//!   [`ark_ports::SessionStore`] and [`ark_ports::EventSink`], plus delegation
//!   depth policy and startup reconciliation.
//! * [`config`] - store configuration and hard resource limits.
//!
//! ## Durability contract
//!
//! `events.jsonl` is the only source of truth. `items.jsonl`, `usage.jsonl`,
//! `session.json`, `children/index.json` and `data/index.json` are all
//! projections: a missing or damaged projection is rebuilt from the log, never
//! the other way round.
//!
//! ## v1 index scope (deliberate deviation from design 7.4.2)
//!
//! The design specifies SQLite (`data/index.sqlite`, WAL, FTS5). v1 ships an
//! in-memory index plus one atomic JSON snapshot at `data/index.json`, because:
//!
//! * it keeps the crate free of a C-compiling dependency (`rusqlite` bundled);
//! * every guarantee that actually matters is expressed on [`ThreadIndex`]
//!   itself - the `indexed_seq` watermark, single-writer projection, and
//!   rebuild-from-directory reconciliation - not on SQL engine behaviour.
//!
//! Single-writer semantics are preserved: only [`IndexProjector`] mutates the
//! index and readers go through a lock. Replacing this module with `rusqlite`
//! later changes neither the [`ark_ports::SessionStore`] surface nor the
//! four-branch reconciliation rule in [`plan_reconcile`].
//!
//! ## Subscribe atomicity
//!
//! [`SessionStore::subscribe`] is answered *inside* the thread's writer actor:
//! the watermark, the `(since_seq, watermark]` backlog and the live channel
//! registration happen in one serialized step, so no append can slip into the
//! gap. Live channels are bounded; a slow consumer is disconnected instead of
//! back-pressuring the writer, and reconnects with its last `seq`.

#![deny(clippy::all)]

pub mod config;
pub mod domain;
pub mod service;
pub mod store;

pub use config::{StoreConfig, StoreLimits};
pub use domain::{
    project_items, ChildIndexEntry, ChildIndexFile, ChildLocator, CompactionMarker, DiagnosticKind,
    DiagnosticSink, DiagnosticSink as StoreDiagnostics, ItemProjector, MemoryDiagnostics,
    SessionState, SessionStatus, StoreDiagnostic, ThreadFile, ThreadPaths,
};
pub use service::{
    reconcile_index, ChildStore, DelegationPolicy, FileSessionStore, ReconcileReport,
};
pub use store::{
    load_snapshot, plan_reconcile, DiskFacts, IndexEntry, IndexProjector, IndexSnapshot, LogParse,
    ReconcileAction, ThreadIndex, WriterHandle,
};
