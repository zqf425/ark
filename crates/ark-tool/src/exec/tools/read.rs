//! Builtin `read` tool: one file slice with line numbers (design §7.3).
//!
//! Output is capped by [`TruncationLimits`], so a huge file cannot flood the
//! model context; the cap is announced in the content instead of silently
//! shortening the result.

use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput};

use crate::domain::{args, schema};
use crate::exec::common::{as_output, read_text, with_detail, workspace_path};
use crate::policy::{cap_all, DEFAULT_LIMITS};

/// Default number of lines returned when the call sets no `limit`.
pub const DEFAULT_LINE_LIMIT: usize = 400;

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
    let text = read_text(&path)?;
    let offset = parsed.offset.unwrap_or(0);
    let limit = parsed.limit.unwrap_or(DEFAULT_LINE_LIMIT).max(1);
    let total = text.lines().count();
    if total > 0 && offset >= total {
        return Ok(ToolOutput::ok(format!(
            "{}: offset {offset} is past the end ({total} lines)",
            display(&path)
        )));
    }
    let body: String = text
        .lines()
        .enumerate()
        .skip(offset)
        .take(limit)
        .map(|(index, line)| format!("{:>6}\t{}\n", index + 1, line))
        .collect();
    let capped = cap_all(&body, &DEFAULT_LIMITS, Some(limit));
    Ok(with_detail(
        ToolOutput::ok(capped),
        format!(
            "{}: lines {}-{} of {total}",
            display(&path),
            offset + 1,
            (offset + limit).min(total)
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

    #[test]
    fn schema_is_the_canonical_read_schema() {
        assert_eq!(ReadTool.schema(), schema::read());
        assert_eq!(ReadTool.name(), "read");
    }
}
