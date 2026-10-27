//! Store diagnostics (design 7.4.1 `index_repaired`, 7.4.3 tail repair).
//!
//! The store never fails because it repaired something; it reports the repair
//! through this small sink so the runtime can surface it as an event.

use ark_common::ThreadId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Why a repair happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticKind {
    TailRepaired,
    IndexRepaired,
    IndexRebuilt,
    IndexCaughtUp,
    IndexInserted,
    IndexRemoved,
    ItemsRebuilt,
    OrphanChildrenCleaned,
    SlowSubscriberDropped,
    LockFileCreated,
}

impl DiagnosticKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticKind::TailRepaired => "tail_repaired",
            DiagnosticKind::IndexRepaired => "index_repaired",
            DiagnosticKind::IndexRebuilt => "index_rebuilt",
            DiagnosticKind::IndexCaughtUp => "index_caught_up",
            DiagnosticKind::IndexInserted => "index_inserted",
            DiagnosticKind::IndexRemoved => "index_removed",
            DiagnosticKind::ItemsRebuilt => "items_rebuilt",
            DiagnosticKind::OrphanChildrenCleaned => "orphan_children_cleaned",
            DiagnosticKind::SlowSubscriberDropped => "slow_subscriber_dropped",
            DiagnosticKind::LockFileCreated => "lock_file_created",
        }
    }
}

/// One diagnostic record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoreDiagnostic {
    pub kind: DiagnosticKind,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    pub at: DateTime<Utc>,
}

impl StoreDiagnostic {
    pub fn new(kind: DiagnosticKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            thread_id: None,
            at: Utc::now(),
        }
    }

    pub fn for_thread(mut self, thread: ThreadId) -> Self {
        self.thread_id = Some(thread);
        self
    }
}

/// Where diagnostics go. The runtime adapts this to a runtime event.
pub trait DiagnosticSink: Send + Sync {
    fn record(&self, diagnostic: StoreDiagnostic);
}

/// Sink that discards everything (useful in tests and in `arkd --check`).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopDiagnostics;

impl DiagnosticSink for NoopDiagnostics {
    fn record(&self, _diagnostic: StoreDiagnostic) {}
}

/// Bounded in-memory sink used by tests and by the startup health report.
#[derive(Debug, Default)]
pub struct MemoryDiagnostics {
    entries: Mutex<Vec<StoreDiagnostic>>,
    capacity: Option<usize>,
}

impl MemoryDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            capacity: Some(capacity.max(1)),
        }
    }

    pub fn entries(&self) -> Vec<StoreDiagnostic> {
        self.entries
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn kinds(&self) -> Vec<DiagnosticKind> {
        self.entries().into_iter().map(|entry| entry.kind).collect()
    }

    pub fn has(&self, kind: DiagnosticKind) -> bool {
        self.entries().iter().any(|entry| entry.kind == kind)
    }
}

impl DiagnosticSink for MemoryDiagnostics {
    fn record(&self, diagnostic: StoreDiagnostic) {
        if let Ok(mut guard) = self.entries.lock() {
            if let Some(capacity) = self.capacity {
                if guard.len() >= capacity {
                    guard.remove(0);
                }
            }
            guard.push(diagnostic);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_have_stable_snake_case_labels() {
        assert_eq!(DiagnosticKind::IndexRepaired.as_str(), "index_repaired");
        assert_eq!(DiagnosticKind::TailRepaired.as_str(), "tail_repaired");
    }

    #[test]
    fn memory_sink_records_and_reports() {
        let sink = MemoryDiagnostics::new();
        sink.record(StoreDiagnostic::new(DiagnosticKind::IndexRepaired, "a"));
        sink.record(StoreDiagnostic::new(DiagnosticKind::ItemsRebuilt, "b"));
        assert_eq!(sink.len(), 2);
        assert!(sink.has(DiagnosticKind::ItemsRebuilt));
        assert!(!sink.has(DiagnosticKind::TailRepaired));
    }

    #[test]
    fn capacity_drops_the_oldest_entry() {
        let sink = MemoryDiagnostics::with_capacity(2);
        for kind in [
            DiagnosticKind::IndexRepaired,
            DiagnosticKind::TailRepaired,
            DiagnosticKind::ItemsRebuilt,
        ] {
            sink.record(StoreDiagnostic::new(kind, "x"));
        }
        assert_eq!(sink.len(), 2);
        assert!(!sink.has(DiagnosticKind::IndexRepaired));
        assert!(sink.has(DiagnosticKind::ItemsRebuilt));
    }
}
