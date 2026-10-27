//! `events.jsonl`: append-only log with tail repair and compaction markers.
//!
//! Design 7.4.3 / 7.4.4. Parsing is a pure function over bytes so crash recovery
//! (half-written last line) is unit-testable without a real crash.

use crate::domain::compaction::CompactionMarker;
use crate::domain::layout::ThreadPaths;
use ark_common::{ArkError, Result};
use ark_contract::RuntimeEvent;
use serde::Serialize;
use std::path::Path;

/// Outcome of parsing an event log.
#[derive(Debug, Clone, Default)]
pub struct LogParse {
    pub events: Vec<RuntimeEvent>,
    pub max_seq: u64,
    /// Bytes to keep when the tail must be repaired (`None` = nothing to cut).
    pub truncate_at: Option<u64>,
    /// Lines inside the file that were not valid events.
    pub malformed: usize,
    /// The last line parsed but has no trailing newline; append must add one.
    pub needs_newline: bool,
}

/// Parse a log buffer, tolerating a torn final line.
///
/// A torn tail is reported through `truncate_at` instead of being silently
/// accepted: the writer must physically cut it before appending, otherwise the
/// next append would be glued onto the damaged bytes.
pub fn parse_log(bytes: &[u8]) -> LogParse {
    let mut out = LogParse::default();
    let split = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|pos| pos + 1);
    let (complete, tail) = match split {
        Some(end) => (&bytes[..end], &bytes[end..]),
        None => (&bytes[..0], bytes),
    };

    for line in complete.split(|byte| *byte == b'\n') {
        count_line(line, &mut out);
    }

    if !tail.is_empty() {
        match serde_json::from_slice::<RuntimeEvent>(tail) {
            Ok(event) => {
                out.needs_newline = true;
                push_event(event, &mut out);
            }
            Err(_) => out.truncate_at = Some(complete.len() as u64),
        }
    }

    if bytes.is_empty() {
        return out;
    }
    out
}

fn count_line(line: &[u8], out: &mut LogParse) {
    if line.iter().all(u8::is_ascii_whitespace) {
        return;
    }
    match serde_json::from_slice::<RuntimeEvent>(line) {
        Ok(event) => push_event(event, out),
        Err(_) => out.malformed += 1,
    }
}

fn push_event(event: RuntimeEvent, out: &mut LogParse) {
    out.max_seq = out.max_seq.max(event.seq());
    out.events.push(event);
}

/// Read the whole log without modifying it.
pub fn read_log_blocking(paths: &ThreadPaths) -> Result<LogParse> {
    let path = paths.events_jsonl();
    match std::fs::read(&path) {
        Ok(bytes) => Ok(parse_log(&bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(LogParse::default()),
        Err(err) => Err(ArkError::store(format!(
            "read event log failed for {}: {err}",
            path.display()
        ))),
    }
}

/// Read the log and physically cut a torn tail. Returns the parse result.
pub fn repair_log_blocking(paths: &ThreadPaths) -> Result<LogParse> {
    let parsed = read_log_blocking(paths)?;
    if let Some(keep) = parsed.truncate_at {
        let path = paths.events_jsonl();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|err| ArkError::store(format!("open for repair failed: {err}")))?;
        file.set_len(keep)
            .map_err(|err| ArkError::store(format!("truncate failed: {err}")))?;
        file.sync_all()
            .map_err(|err| ArkError::store(format!("truncate fsync failed: {err}")))?;
    }
    Ok(parsed)
}

/// Encode one event as a JSONL record (with trailing newline).
pub fn encode_event(event: &RuntimeEvent) -> Result<String> {
    let mut line = serde_json::to_string(event)?;
    line.push('\n');
    Ok(line)
}

/// Encode `value` as one JSONL record (with trailing newline).
pub fn encode_line<T: Serialize>(value: &T) -> Result<String> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    Ok(line)
}

/// Append a prepared line to a JSONL file, fsyncing according to `fsync`.
pub fn append_text(path: &Path, text: &str, fsync: bool) -> Result<()> {
    crate::store::fs_util::append_line_blocking(path, text, fsync)
}

/// Read compaction markers; a damaged marker file never fails a replay.
///
/// Decoding goes through the shared policy so a BOM-prefixed marker file cannot
/// turn every marker into an unparsable line.
pub fn read_markers_blocking(paths: &ThreadPaths) -> Vec<CompactionMarker> {
    ark_common::read_text_opt(&paths.compacted_jsonl())
        .map(|text| {
            text.lines()
                .filter_map(crate::domain::compaction::parse_marker)
                .collect()
        })
        .unwrap_or_default()
}

/// Compute the compacted log body and the marker describing what was folded.
///
/// `keep_last` most recent events survive; everything strictly older becomes a
/// declared hole. Returns `None` when there is nothing worth folding.
pub fn plan_compaction(
    events: &[RuntimeEvent],
    keep_last: usize,
    reason: &str,
) -> Result<Option<(String, CompactionMarker)>> {
    if events.len() <= keep_last.max(1) {
        return Ok(None);
    }
    let keep = keep_last.max(1).min(events.len());
    let cut = events.len() - keep;
    let folded = &events[..cut];
    let from_seq = folded.first().map(RuntimeEvent::seq).unwrap_or(0);
    let to_seq = folded.last().map(RuntimeEvent::seq).unwrap_or(0);
    let mut body = String::new();
    for event in &events[cut..] {
        body.push_str(&encode_event(event)?);
    }
    let marker = CompactionMarker::new(from_seq, to_seq, keep as u64, reason);
    Ok(Some((body, marker)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::{event_at, TempDir};
    use ark_common::ThreadId;

    fn write(paths: &ThreadPaths, text: &str) {
        crate::store::fs_util::ensure_dir(paths.dir()).unwrap();
        std::fs::write(paths.events_jsonl(), text).unwrap();
    }

    #[test]
    fn parses_three_events_and_reports_max_seq() {
        let thread = ThreadId::new();
        let mut text = String::new();
        for seq in 1..=3 {
            text.push_str(&encode_event(&event_at(thread, seq)).unwrap());
        }
        let parsed = parse_log(text.as_bytes());
        assert_eq!(parsed.events.len(), 3);
        assert_eq!(parsed.max_seq, 3);
        assert!(parsed.truncate_at.is_none());
        assert_eq!(parsed.malformed, 0);
    }

    #[test]
    fn torn_tail_is_reported_for_truncation() {
        let thread = ThreadId::new();
        let mut text = String::new();
        for seq in 1..=2 {
            text.push_str(&encode_event(&event_at(thread, seq)).unwrap());
        }
        let keep = text.len() as u64;
        text.push_str("{\"kind\":\"turn_star");
        let parsed = parse_log(text.as_bytes());
        assert_eq!(parsed.events.len(), 2);
        assert_eq!(parsed.max_seq, 2);
        assert_eq!(parsed.truncate_at, Some(keep));
    }

    #[test]
    fn repair_drops_the_half_line_and_append_continues() {
        let dir = TempDir::new("log-repair");
        let thread = ThreadId::new();
        let paths = ThreadPaths::new(dir.path().join("thread"));
        // Reuse the exact encoded lines: re-encoding would mint new timestamps
        // and make the byte-length assertion depend on the clock.
        let first = encode_event(&event_at(thread, 1)).unwrap();
        let second = encode_event(&event_at(thread, 2)).unwrap();
        let mut text = String::new();
        text.push_str(&first);
        text.push_str(&second);
        text.push_str("{\"kind\":\"turn_star");
        write(&paths, &text);

        let repaired = repair_log_blocking(&paths).unwrap();
        assert_eq!(repaired.max_seq, 2);
        let expected_len = (first.len() + second.len()) as u64;
        assert_eq!(
            crate::store::fs_util::file_len(&paths.events_jsonl()),
            expected_len
        );

        append_text(
            &paths.events_jsonl(),
            &encode_event(&event_at(thread, 3)).unwrap(),
            true,
        )
        .unwrap();
        let after = read_log_blocking(&paths).unwrap();
        assert_eq!(after.events.len(), 3);
        assert_eq!(after.max_seq, 3);
    }

    #[test]
    fn missing_log_reads_as_empty() {
        let dir = TempDir::new("log-missing");
        let paths = ThreadPaths::new(dir.path().join("thread"));
        let parsed = read_log_blocking(&paths).unwrap();
        assert!(parsed.events.is_empty());
        assert_eq!(parsed.max_seq, 0);
    }

    #[test]
    fn garbage_in_the_middle_is_skipped_but_kept_on_disk() {
        let dir = TempDir::new("log-garbage");
        let thread = ThreadId::new();
        let paths = ThreadPaths::new(dir.path().join("thread"));
        let mut text = String::new();
        text.push_str(&encode_event(&event_at(thread, 1)).unwrap());
        text.push_str("garbage\n");
        text.push_str(&encode_event(&event_at(thread, 2)).unwrap());
        write(&paths, &text);

        let parsed = read_log_blocking(&paths).unwrap();
        assert_eq!(parsed.events.len(), 2);
        assert_eq!(parsed.malformed, 1);
        assert!(
            parsed.truncate_at.is_none(),
            "mid-file garbage is not a torn tail"
        );
    }

    #[test]
    fn compaction_folds_old_events_and_declares_the_hole() {
        let thread = ThreadId::new();
        let events: Vec<RuntimeEvent> = (1..=10).map(|seq| event_at(thread, seq)).collect();
        let (body, marker) = plan_compaction(&events, 4, "capacity").unwrap().unwrap();
        assert_eq!(marker.from_seq, 1);
        assert_eq!(marker.to_seq, 6);
        assert_eq!(marker.kept, 4);
        let reparsed = parse_log(body.as_bytes());
        assert_eq!(reparsed.events.len(), 4);
        assert_eq!(reparsed.max_seq, 10);
        assert!(marker.contains(3));
        assert!(!marker.contains(9));
    }

    #[test]
    fn compaction_is_a_no_op_when_under_the_limit() {
        let thread = ThreadId::new();
        let events: Vec<RuntimeEvent> = (1..=3).map(|seq| event_at(thread, seq)).collect();
        assert!(plan_compaction(&events, 8, "test").unwrap().is_none());
    }
}
