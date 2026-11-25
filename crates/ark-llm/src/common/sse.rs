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

/// Maximum bytes in one SSE frame, including its blank-line terminator.
pub const MAX_SSE_FRAME_BYTES: usize = 1024 * 1024;

/// Frames completed by one feed. `made_progress` includes comments and empty
/// frames: transport liveness is about receiving a complete SSE frame, not
/// about whether that frame happened to contain model content.
#[derive(Debug, Default)]
pub struct SseBatch {
    pub events: Vec<SseEvent>,
    pub made_progress: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseError {
    FrameTooLarge { limit: usize },
    UnterminatedFrame { pending: usize },
}

impl std::fmt::Display for SseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FrameTooLarge { limit } => {
                write!(f, "SSE frame exceeded the {limit}-byte limit")
            }
            Self::UnterminatedFrame { pending } => {
                write!(
                    f,
                    "SSE stream ended with an unterminated {pending}-byte frame"
                )
            }
        }
    }
}

impl std::error::Error for SseError {}

/// Streaming SSE parser. Owns only a bounded incomplete tail, never the body.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
    failed: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one raw chunk and return every complete frame it dispatched.
    pub fn push(&mut self, mut chunk: &str) -> Result<SseBatch, SseError> {
        if self.failed {
            return Err(SseError::FrameTooLarge {
                limit: MAX_SSE_FRAME_BYTES,
            });
        }
        let mut batch = SseBatch::default();
        while !chunk.is_empty() {
            let room = MAX_SSE_FRAME_BYTES + 3 - self.buffer.len();
            let mut end = chunk.len().min(room);
            while !chunk.is_char_boundary(end) {
                end -= 1;
            }
            if end == 0 {
                return Err(self.fail());
            }
            self.buffer.push_str(&chunk[..end]);
            chunk = &chunk[end..];
            self.drain(&mut batch)?;
            if self.buffer.len() > MAX_SSE_FRAME_BYTES {
                return Err(self.fail());
            }
        }
        Ok(batch)
    }

    /// Validate EOF without dispatching an unterminated frame. A missing blank
    /// line is a protocol truncation, not permission to invent a final event.
    pub fn finish(&self) -> Result<(), SseError> {
        if self.failed {
            return Err(SseError::FrameTooLarge {
                limit: MAX_SSE_FRAME_BYTES,
            });
        }
        if self.buffer.is_empty() {
            Ok(())
        } else {
            Err(SseError::UnterminatedFrame {
                pending: self.buffer.len(),
            })
        }
    }

    /// Bytes buffered but not yet dispatched. Used by tests and diagnostics.
    pub fn pending(&self) -> &str {
        &self.buffer
    }

    fn drain(&mut self, batch: &mut SseBatch) -> Result<(), SseError> {
        while let Some(separator) = find_separator(&self.buffer) {
            if separator.end > MAX_SSE_FRAME_BYTES {
                return Err(self.fail());
            }
            let frame = self.buffer[..separator.start].to_string();
            self.buffer.drain(..separator.end);
            batch.made_progress = true;
            if let Some(event) = parse_frame(&frame) {
                batch.events.push(event);
            }
        }
        Ok(())
    }

    fn fail(&mut self) -> SseError {
        self.failed = true;
        self.buffer.clear();
        SseError::FrameTooLarge {
            limit: MAX_SSE_FRAME_BYTES,
        }
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
            // Only step back over the opening newline when it is itself
            // CR-terminated. A bare `\n\r\n` (LF data line + CRLF blank line)
            // must keep `start` at `index`: stepping back unconditionally both
            // dropped the last data byte and, when index == 0, underflowed
            // `index - 1` into a wrapped slice start.
            let start = if index > 0 && bytes[index - 1] == b'\r' {
                index - 1
            } else {
                index
            };
            return Some(Separator {
                start,
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

    fn feed(parser: &mut SseParser, chunk: &str) -> Vec<SseEvent> {
        parser.push(chunk).unwrap().events
    }

    #[test]
    fn one_frame_in_one_chunk() {
        let mut parser = SseParser::new();
        let events = feed(&mut parser, "data: {\"a\":1}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "{\"a\":1}");
        assert!(parser.pending().is_empty());
    }

    #[test]
    fn a_frame_split_across_chunks_is_reassembled() {
        let mut parser = SseParser::new();
        assert!(feed(&mut parser, "data: {\"hel").is_empty());
        let events = feed(&mut parser, "lo\":1}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "{\"hello\":1}");
    }

    #[test]
    fn several_frames_in_one_chunk() {
        let mut parser = SseParser::new();
        let events = feed(&mut parser, "data: a\n\ndata: b\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "a");
        assert_eq!(events[1].data, "b");
    }

    #[test]
    fn crlf_terminators_are_supported() {
        let mut parser = SseParser::new();
        let events = feed(&mut parser, "event: delta\r\ndata: x\r\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name(), "delta");
        assert_eq!(events[0].data, "x");
    }

    #[test]
    fn multi_line_data_is_joined_with_newlines() {
        let mut parser = SseParser::new();
        let events = feed(&mut parser, "data: line1\ndata: line2\n\n");
        assert_eq!(events[0].data, "line1\nline2");
    }

    #[test]
    fn comment_lines_are_ignored() {
        let mut parser = SseParser::new();
        let events = feed(&mut parser, ": keep-alive\n\ndata: real\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "real");
    }

    #[test]
    fn done_sentinel_is_recognised() {
        let mut parser = SseParser::new();
        let events = feed(&mut parser, "data: [DONE]\n\n");
        assert!(events[0].is_done());
    }

    #[test]
    fn trailing_frame_is_rejected_on_finish() {
        let mut parser = SseParser::new();
        assert!(feed(&mut parser, "data: {\"usage\":1}").is_empty());
        assert!(matches!(
            parser.finish(),
            Err(SseError::UnterminatedFrame { .. })
        ));
    }

    #[test]
    fn finish_without_pending_yields_nothing() {
        let mut parser = SseParser::new();
        feed(&mut parser, "data: a\n\n");
        assert!(parser.finish().is_ok());
    }

    #[test]
    fn empty_frames_are_skipped() {
        let mut parser = SseParser::new();
        assert!(feed(&mut parser, "\n\n").is_empty());
    }

    #[test]
    fn buffer_starting_with_lf_crlf_crlf_does_not_underflow() {
        let mut parser = SseParser::new();
        let events = feed(&mut parser, "\n\r\ndata: x\r\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "x");
    }

    #[test]
    fn lf_data_line_before_crlf_blank_line_keeps_its_last_byte() {
        let mut parser = SseParser::new();
        let events = feed(&mut parser, "data: a\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "a");
    }

    #[test]
    fn crlf_separator_split_across_chunks_is_reassembled() {
        let mut parser = SseParser::new();
        assert!(feed(&mut parser, "data: x\r").is_empty());
        let events = feed(&mut parser, "\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "x");
    }

    #[test]
    fn field_without_value_is_tolerated() {
        let mut parser = SseParser::new();
        let events = feed(&mut parser, "data\ndata: value\n\n");
        assert_eq!(events[0].data, "\nvalue");
    }

    #[test]
    fn incomplete_frame_buffer_is_bounded() {
        let mut parser = SseParser::new();
        let chunk = format!("data: {}", "x".repeat(1_048_577));
        assert!(matches!(
            parser.push(&chunk),
            Err(SseError::FrameTooLarge { .. })
        ));
        assert!(parser.pending().is_empty());
    }

    #[test]
    fn oversized_completed_frame_is_rejected() {
        let mut parser = SseParser::new();
        let chunk = format!("data: {}\n\n", "x".repeat(1_048_577));
        assert!(matches!(
            parser.push(&chunk),
            Err(SseError::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn comment_only_frames_count_as_transport_progress() {
        let mut parser = SseParser::new();
        let batch = parser.push(": keep-alive\n\n").unwrap();
        assert!(batch.made_progress);
        assert!(batch.events.is_empty());
    }
}
