//! Hook audit records (requirements/plugin-hook-bus.md, 审计).
//!
//! A plugin decision must answer "why was this refused / retuned" after the
//! fact, so every hook invocation the broker serves lands here as a record.
//! The shape deliberately mirrors ark-http's `HttpDiagnosticSink`: a bounded
//! in-memory ring plus a wire view on `GET /v1/runtime/plugins` — never a
//! print, and never an unbounded log.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// One audited hook invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookAuditRecord {
    /// `hook_decide` for a synchronous verdict, `hook_observe` for a
    /// notification the broker delivered.
    pub kind: String,
    pub plugin_id: String,
    /// Hook point name, e.g. `llm.request` or `tool.pre_call`.
    pub hook_point: String,
    /// What the plugin decided, e.g. `allow`, `block`, `tune`, `abstain`.
    pub decision: String,
    /// Human-readable diff or reason (payload summary already redacted by the
    /// caller; this field never carries full tool arguments or messages).
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub duration_ms: u64,
}

/// The audit sink port. The hook broker writes; the plugin host exposes.
pub trait HookAuditSink: Send + Sync {
    fn record(&self, record: HookAuditRecord);
}

/// Bounded in-memory ring, newest last. A dropped record is a capacity event,
/// not a correctness one: decisions are advisory evidence, not state.
#[derive(Debug)]
pub struct InMemoryHookAudit {
    capacity: usize,
    records: Mutex<Vec<HookAuditRecord>>,
}

impl InMemoryHookAudit {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            records: Mutex::new(Vec::new()),
        }
    }

    /// All retained records, oldest first.
    pub fn records(&self) -> Vec<HookAuditRecord> {
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl HookAuditSink for InMemoryHookAudit {
    fn record(&self, record: HookAuditRecord) {
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        records.push(record);
        if self.capacity > 0 && records.len() > self.capacity {
            let excess = records.len() - self.capacity;
            records.drain(0..excess);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(n: u8) -> HookAuditRecord {
        HookAuditRecord {
            kind: "hook_decide".into(),
            plugin_id: "ark.example.life".into(),
            hook_point: "tool.pre_call".into(),
            decision: "block".into(),
            detail: format!("call {n}"),
            duration_ms: 3,
        }
    }

    #[test]
    fn the_ring_keeps_only_the_newest_records() {
        let sink = InMemoryHookAudit::new(2);
        for n in 0..4u8 {
            sink.record(record(n));
        }
        let kept: Vec<String> = sink
            .records()
            .into_iter()
            .map(|record| record.detail)
            .collect();
        assert_eq!(kept, vec!["call 2", "call 3"]);
    }
}
