//! The four model-facing tools, published as `<plugin_id>.<tool>`.
//!
//! Every handler answers through `McpCallResult`; a refusal is `ok: false`
//! with a detail code rather than an `Err`, because a model can act on a
//! structured refusal but not on a transport error.

use std::sync::Arc;

use ark_common::Result;
use ark_mcp::{CallContext, McpHandler};
use ark_ports::McpCallResult;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::store::MemoryStore;

/// Cap on a caller-supplied page size. A model asking for 10 000 results would
/// push the whole store into one tool result and blow its own context.
const MAX_LIMIT: usize = 50;
const DEFAULT_LIMIT: usize = 10;

/// `memory_store`: persist one memory.
pub struct StoreTool {
    store: Arc<MemoryStore>,
}

impl StoreTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl McpHandler for StoreTool {
    async fn call(&self, _ctx: &CallContext, args: Value) -> Result<McpCallResult> {
        let Some(text) = args.get("text").and_then(Value::as_str) else {
            return Ok(refuse("memory_store requires a `text` string"));
        };
        let tags = string_list(args.get("tags"));
        let source = args.get("source").and_then(Value::as_str);
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(next_id);
        let created_at = now_rfc3339();

        match self.store.store(&id, text, &tags, source, &created_at) {
            Ok(record) => Ok(McpCallResult {
                ok: true,
                content: json!({
                    "id": record.id,
                    "created_at": record.created_at,
                    "tags": record.tags,
                }),
                trace_id: String::new(),
            }),
            // A validation refusal (empty or oversized text, bad id) is the
            // model's mistake to fix, so it comes back as `ok: false`.
            Err(err) => Ok(refuse(err.message)),
        }
    }
}

/// `memory_search`: lexical recall with an optional tag filter.
pub struct SearchTool {
    store: Arc<MemoryStore>,
}

impl SearchTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl McpHandler for SearchTool {
    async fn call(&self, _ctx: &CallContext, args: Value) -> Result<McpCallResult> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let tags = string_list(args.get("tags"));
        let limit = limit_from(&args);
        let hits = self.store.search(query, &tags, limit);
        Ok(McpCallResult {
            ok: true,
            content: json!({ "count": hits.len(), "memories": hits }),
            trace_id: String::new(),
        })
    }
}

/// `memory_list`: newest-first page of everything stored.
pub struct ListTool {
    store: Arc<MemoryStore>,
}

impl ListTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl McpHandler for ListTool {
    async fn call(&self, _ctx: &CallContext, args: Value) -> Result<McpCallResult> {
        let limit = limit_from(&args);
        let mut all = self.store.list();
        let total = all.len();
        all.truncate(limit);
        Ok(McpCallResult {
            ok: true,
            content: json!({ "total": total, "count": all.len(), "memories": all }),
            trace_id: String::new(),
        })
    }
}

/// `memory_delete`: tombstone one memory.
pub struct DeleteTool {
    store: Arc<MemoryStore>,
}

impl DeleteTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl McpHandler for DeleteTool {
    async fn call(&self, _ctx: &CallContext, args: Value) -> Result<McpCallResult> {
        let Some(id) = args.get("id").and_then(Value::as_str) else {
            return Ok(refuse("memory_delete requires an `id` string"));
        };
        match self.store.delete(id) {
            Ok(true) => Ok(McpCallResult {
                ok: true,
                content: json!({ "deleted": id }),
                trace_id: String::new(),
            }),
            Ok(false) => Ok(refuse(format!("memory `{id}` was not found"))),
            Err(err) => Ok(refuse(err.message)),
        }
    }
}

fn refuse(message: impl Into<String>) -> McpCallResult {
    McpCallResult {
        ok: false,
        content: json!({ "error": message.into() }),
        trace_id: String::new(),
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn limit_from(args: &Value) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map(|raw| raw as usize)
        .filter(|raw| *raw > 0)
        .map(|raw| raw.min(MAX_LIMIT))
        .unwrap_or(DEFAULT_LIMIT)
}

/// Ids are opaque handles the caller passes back; a counter plus the clock is
/// enough and keeps the crate free of a UUID dependency.
fn next_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("mem_{stamp}_{seq}")
}

/// RFC 3339 UTC without pulling in a date library: the store only needs a
/// lexicographically sortable stamp, and this crate's dependency list is part
/// of its isolation guarantee.
fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's days-to-civil conversion, the standard branch-free form.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generated_timestamp_is_sorted_safe_and_well_formed() {
        let stamp = now_rfc3339();
        assert_eq!(stamp.len(), 20, "{stamp}");
        assert!(stamp.ends_with('Z'));
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "T");
    }

    #[test]
    fn civil_dates_match_known_epochs() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
    }

    #[test]
    fn limits_are_clamped_to_the_documented_ceiling() {
        assert_eq!(limit_from(&json!({})), DEFAULT_LIMIT);
        assert_eq!(limit_from(&json!({ "limit": 0 })), DEFAULT_LIMIT);
        assert_eq!(limit_from(&json!({ "limit": 5 })), 5);
        assert_eq!(limit_from(&json!({ "limit": 10_000 })), MAX_LIMIT);
    }

    #[test]
    fn ids_are_unique_across_calls() {
        assert_ne!(next_id(), next_id());
    }
}
