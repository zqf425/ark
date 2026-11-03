//! Builtin `read` tool: one file slice with line numbers (design §7.3).
//!
//! The file is streamed line by line so a huge log never enters memory whole:
//! only the requested `[offset, offset + limit)` window is kept. Output is
//! still capped by the shared truncation limits, and the cap is announced in
//! the content instead of silently shortening the result.

use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput};

use crate::domain::{args, schema};
use crate::exec::common::{as_output, read_text, with_detail, workspace_path};
use crate::policy::{DEFAULT_LIMITS, cap_all};

/// Default number of lines returned when the call sets no `limit`.
pub const DEFAULT_LINE_LIMIT: usize = 400;

/// Line window selected by one `read` call: the formatted body plus the
/// total line count seen while scanning, so the detail line can report
/// `lines X-Y of total` without a second pass.
struct LineWindow {
    body: String,
    total: usize,
    offset: usize,
    limit: usize,
}

impl LineWindow {
    fn new(offset: usize, limit: usize) -> Self {
        Self {
            body: String::new(),
            total: 0,
            offset,
            limit,
        }
    }

    /// Count the line and keep it only when it falls inside the window.
    fn push(&mut self, line: &str) {
        if self.total >= self.offset && self.total < self.offset.saturating_add(self.limit) {
            self.body
                .push_str(&format!("{:>6}\t{}\n", self.total + 1, line));
        }
        self.total += 1;
    }
}

/// Collect the numbered `[offset, offset + limit)` window in one pass.
///
/// UTF-8 files stream through [`ark_common::try_stream_utf8_lines`], so peak
/// memory is the longest line, not the file. When the bytes are not valid
/// UTF-8, the file is redone through the whole-file fallback chain
/// (GB18030, Big5, ...) instead of emitting replacement characters; the
/// partially collected window is discarded first, so lines never double up.
fn collect_window(
    path: &std::path::Path,
    offset: usize,
    limit: usize,
) -> ark_common::Result<LineWindow> {
    let mut window = LineWindow::new(offset, limit);
    if ark_common::try_stream_utf8_lines(path, &mut |line| window.push(line))? {
        return Ok(window);
    }
    let mut window = LineWindow::new(offset, limit);
    for line in read_text(path)?.lines() {
        window.push(line);
    }
    Ok(window)
}

/// `read`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadTool;

impl ReadTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn schema(&self) -> &'static serde_json::Value {
        schema::read()
    }

    async fn execute(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        Ok(as_output(run(ctx, raw)))
    }
}

fn run(ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
    let parsed: args::ReadArgs = args::parse("read", &raw)?;
    let path = workspace_path(ctx, Some(&parsed.path))?;
    let offset = parsed.offset.unwrap_or(0);
    let limit = parsed.limit.unwrap_or(DEFAULT_LINE_LIMIT).max(1);
    let window = collect_window(&path, offset, limit)?;
    if window.total > 0 && offset >= window.total {
        return Ok(ToolOutput::ok(format!(
            "{}: offset {offset} is past the end ({} lines)",
            display(&path),
            window.total
        )));
    }
    let capped = cap_all(&window.body, &DEFAULT_LIMITS, Some(limit));
    Ok(with_detail(
        ToolOutput::ok(capped),
        format!(
            "{}: lines {}-{} of {}",
            display(&path),
            offset + 1,
            offset.saturating_add(limit).min(window.total),
            window.total
        ),
    ))
}

fn display(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::testsupport::{ctx, temp_workspace};

    #[tokio::test]
    async fn reads_a_slice_with_line_numbers() {
        let dir = temp_workspace("read-slice");
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let raw = ToolArgs(serde_json::json!({ "path": "a.txt", "offset": 1, "limit": 1 }));
        let out = ReadTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok);
        assert!(out.content.contains("2\ttwo"), "got {}", out.content);
        assert!(!out.content.contains("one"));
    }

    #[tokio::test]
    async fn missing_file_is_reported_without_panicking() {
        let dir = temp_workspace("read-missing");
        let raw = ToolArgs(serde_json::json!({ "path": "nope.txt" }));
        let out = ReadTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("cannot read"));
    }

    #[tokio::test]
    async fn escape_is_refused() {
        let dir = temp_workspace("read-escape");
        let raw = ToolArgs(serde_json::json!({ "path": "../../../etc/passwd" }));
        let out = ReadTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("escapes"));
    }

    #[tokio::test]
    async fn offset_past_the_end_says_so() {
        let dir = temp_workspace("read-offset");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        let raw = ToolArgs(serde_json::json!({ "path": "a.txt", "offset": 50 }));
        let out = ReadTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok);
        assert!(out.content.contains("past the end"));
    }

    #[tokio::test]
    async fn large_file_returns_only_the_requested_window() {
        // Regression: `read` used to load the whole file before slicing, so a
        // huge log landed in memory in full. The total must still be exact.
        let dir = temp_workspace("read-large");
        let body: String = (0..50_000).map(|index| format!("line-{index}\n")).collect();
        std::fs::write(dir.join("big.log"), body.as_bytes()).unwrap();
        let raw = ToolArgs(serde_json::json!({ "path": "big.log", "offset": 49_998, "limit": 5 }));
        let out = ReadTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok);
        assert!(
            out.content.contains("49999\tline-49998"),
            "got {}",
            out.content
        );
        assert!(
            out.content.contains("50000\tline-49999"),
            "got {}",
            out.content
        );
        assert!(!out.content.contains("\tline-0\n"));
        assert_eq!(
            out.detail.as_deref(),
            Some("big.log: lines 49999-50000 of 50000")
        );
    }

    #[tokio::test]
    async fn multibyte_characters_survive_the_line_reader() {
        // Regression guard: line reads must never split a multi-byte UTF-8
        // sequence into replacement characters.
        let dir = temp_workspace("read-multibyte");
        std::fs::write(dir.join("cjk.txt"), "第一行\n第二行\n").unwrap();
        let raw = ToolArgs(serde_json::json!({ "path": "cjk.txt" }));
        let out = ReadTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok);
        assert!(out.content.contains("第一行"), "got {}", out.content);
        assert!(out.content.contains("第二行"), "got {}", out.content);
        assert!(!out.content.contains('\u{FFFD}'));
    }

    #[tokio::test]
    async fn legacy_encoded_file_still_reads_through_the_fallback_chain() {
        // "中文" in GBK is not valid UTF-8; the whole-file fallback chain must
        // still decode it exactly like the previous whole-file read did.
        let dir = temp_workspace("read-gbk");
        std::fs::write(dir.join("gbk.txt"), [0xD6_u8, 0xD0, 0xCE, 0xC4, b'\n']).unwrap();
        let raw = ToolArgs(serde_json::json!({ "path": "gbk.txt" }));
        let out = ReadTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok);
        assert!(out.content.contains("中文"), "got {}", out.content);
        assert!(!out.content.contains('\u{FFFD}'));
    }

    #[test]
    fn schema_is_the_canonical_read_schema() {
        assert_eq!(ReadTool.schema(), schema::read());
        assert_eq!(ReadTool.name(), "read");
    }
}
