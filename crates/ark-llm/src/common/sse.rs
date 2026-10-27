//! SSE frame parsing (design §7.2 `common/stream.rs`).
//!
//! Incremental by design: bytes arrive in arbitrary chunks, a frame may be split
//! across two reads, and the parser must never buffer the whole reply. Frames
//! are handed back as soon as a blank line completes them.

/// One dispatched SSE frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseEvent {
    /// `event:` field, when the provider names its frames.
    pub event: Option<String>,
    /// Concatenated `data:` lines joined with `\n`.
    pub data: String,
}

impl SseEvent {
    /// True for the OpenAI `[DONE]` sentinel.
    pub fn is_done(&self) -> bool {
        self.data.trim() == "[DONE]"
    }

    pub fn name(&self) -> &str {
        self.event.as_deref().unwrap_or("")
    }
}

/// Streaming SSE parser. Owns only the incomplete tail, never the full body.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one raw chunk and return every frame it completed.
    pub fn push(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();
        while let Some(frame) = self.take_frame() {
            if let Some(event) = parse_frame(&frame) {
                events.push(event);
            }
        }
        events
    }

    /// Flush a trailing frame that never got its blank-line terminator.
    ///
    /// A provider that closes the connection right after the last `data:` line
    /// would otherwise lose that final frame, which is exactly where usage
    /// usually lives.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let frame = std::mem::take(&mut self.buffer);
        match parse_frame(&frame) {
            Some(event) => vec![event],
            None => Vec::new(),
        }
    }

    /// Bytes buffered but not yet dispatched. Used by tests and diagnostics.
    pub fn pending(&self) -> &str {
        &self.buffer
    }

    fn take_frame(&mut self) -> Option<String> {
        let separator = find_separator(&self.buffer)?;
        let frame = self.buffer[..separator.start].to_string();
        self.buffer.drain(..separator.end);
        Some(frame)
    }
}

struct Separator {
    start: usize,
    end: usize,
}

/// Find the end of the first frame: `\n\n`, `\r\n\r\n` or a mixed variant.
fn find_separator(buffer: &str) -> Option<Separator> {
    let bytes = buffer.as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] != b'\n' {
            continue;
        }
        // Case 1: "\n\n" — the blank line starts here.
        if bytes.get(index + 1) == Some(&b'\n') {
            return Some(Separator {
                start: index,
                end: index + 2,
            });
        }
        // Case 2: "\r\n\r\n" — end the frame before the CRLF pair.
        if bytes.get(index + 1) == Some(&b'\r') && bytes.get(index + 2) == Some(&b'\n') {
            return Some(Separator {
                start: index - 1,
                end: index + 3,
            });
        }
    }
    None
}

fn parse_frame(frame: &str) -> Option<SseEvent> {
    let mut event = SseEvent::default();
    let mut has_data = false;
    for line in frame.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "data" => {
                if has_data {
                    event.data.push('\n');
                }
                event.data.push_str(value);
                has_data = true;
            }
            "event" => event.event = Some(value.to_string()),
            // `id` and `retry` carry no model content; ignoring them keeps the
            // frame shape identical across providers.
            _ => {}
        }
    }
    if has_data || event.event.is_some() {
        Some(event)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_frame_in_one_chunk() {
        let mut parser = SseParser::new();
        let events = parser.push("data: {\"a\":1}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "{\"a\":1}");
        assert!(parser.pending().is_empty());
    }

    #[test]
    fn a_frame_split_across_chunks_is_reassembled() {
        let mut parser = SseParser::new();
        assert!(parser.push("data: {\"hel").is_empty());
        let events = parser.push("lo\":1}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "{\"hello\":1}");
    }

    #[test]
    fn several_frames_in_one_chunk() {
        let mut parser = SseParser::new();
        let events = parser.push("data: a\n\ndata: b\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "a");
        assert_eq!(events[1].data, "b");
    }

    #[test]
    fn crlf_terminators_are_supported() {
        let mut parser = SseParser::new();
        let events = parser.push("event: delta\r\ndata: x\r\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name(), "delta");
        assert_eq!(events[0].data, "x");
    }

    #[test]
    fn multi_line_data_is_joined_with_newlines() {
        let mut parser = SseParser::new();
        let events = parser.push("data: line1\ndata: line2\n\n");
        assert_eq!(events[0].data, "line1\nline2");
    }

    #[test]
    fn comment_lines_are_ignored() {
        let mut parser = SseParser::new();
        let events = parser.push(": keep-alive\n\ndata: real\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "real");
    }

    #[test]
    fn done_sentinel_is_recognised() {
        let mut parser = SseParser::new();
        let events = parser.push("data: [DONE]\n\n");
        assert!(events[0].is_done());
    }

    #[test]
    fn trailing_frame_is_flushed_on_finish() {
        let mut parser = SseParser::new();
        assert!(parser.push("data: {\"usage\":1}").is_empty());
        let events = parser.finish();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "{\"usage\":1}");
    }

    #[test]
    fn finish_without_pending_yields_nothing() {
        let mut parser = SseParser::new();
        parser.push("data: a\n\n");
        assert!(parser.finish().is_empty());
    }

    #[test]
    fn empty_frames_are_skipped() {
        let mut parser = SseParser::new();
        assert!(parser.push("\n\n").is_empty());
    }

    #[test]
    fn field_without_value_is_tolerated() {
        let mut parser = SseParser::new();
        let events = parser.push("data\ndata: value\n\n");
        assert_eq!(events[0].data, "\nvalue");
    }
}
