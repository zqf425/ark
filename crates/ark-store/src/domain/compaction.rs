//! Compaction markers: an explicit line declares that a `seq` range was folded.
//!
//! Design 7.4.4. Kept beside the event log in `events.jsonl.compacted` so the
//! log itself only ever contains real events. A subscriber reading a hole must
//! skip it silently, because `seq` monotonicity is the only ordering contract.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One folded range `[from_seq, to_seq]` (inclusive on both ends).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionMarker {
    pub from_seq: u64,
    pub to_seq: u64,
    pub compacted_at: DateTime<Utc>,
    #[serde(default)]
    pub kept: u64,
    #[serde(default)]
    pub reason: String,
}

impl CompactionMarker {
    pub fn new(from_seq: u64, to_seq: u64, kept: u64, reason: impl Into<String>) -> Self {
        Self {
            from_seq,
            to_seq,
            compacted_at: Utc::now(),
            kept,
            reason: reason.into(),
        }
    }

    pub fn contains(&self, seq: u64) -> bool {
        seq >= self.from_seq && seq <= self.to_seq
    }

    /// Serialized marker line, without the trailing newline.
    pub fn to_line(&self) -> String {
        let payload = serde_json::json!({
            "kind": "compaction",
            "from_seq": self.from_seq,
            "to_seq": self.to_seq,
            "compacted_at": self.compacted_at,
            "kept": self.kept,
            "reason": self.reason,
        });
        payload.to_string()
    }
}

/// Parse one `events.jsonl.compacted` line.
///
/// Returns `None` for blank lines and for anything that is not a marker, so a
/// damaged marker file cannot fail a replay.
pub fn parse_marker(line: &str) -> Option<CompactionMarker> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    if value.get("kind").and_then(|kind| kind.as_str()) != Some("compaction") {
        return None;
    }
    let marker: CompactionMarker = serde_json::from_value(value).ok()?;
    if marker.to_seq < marker.from_seq {
        return None;
    }
    Some(marker)
}

/// Whether `seq` was folded away and therefore has no event line any more.
pub fn is_compacted(seq: u64, markers: &[CompactionMarker]) -> bool {
    markers.iter().any(|marker| marker.contains(seq))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_line_roundtrips() {
        let marker = CompactionMarker::new(1, 10, 8, "capacity");
        let parsed = parse_marker(&marker.to_line()).expect("marker parses");
        assert_eq!(parsed.from_seq, 1);
        assert_eq!(parsed.to_seq, 10);
        assert_eq!(parsed.kept, 8);
        assert_eq!(parsed.reason, "capacity");
    }

    #[test]
    fn non_marker_lines_are_ignored() {
        assert!(parse_marker("").is_none());
        assert!(parse_marker("not json").is_none());
        assert!(parse_marker("{\"kind\":\"usage\"}").is_none());
        assert!(parse_marker("{\"kind\":\"compaction\",\"from_seq\":9,\"to_seq\":1}").is_none());
    }

    #[test]
    fn hole_check_covers_the_inclusive_range() {
        let markers = vec![CompactionMarker::new(3, 5, 2, "test")];
        assert!(!is_compacted(2, &markers));
        assert!(is_compacted(3, &markers));
        assert!(is_compacted(5, &markers));
        assert!(!is_compacted(6, &markers));
    }
}
