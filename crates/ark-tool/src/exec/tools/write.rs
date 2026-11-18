//! Builtin `write` and `edit` tools (design §7.3).
//!
//! Both mutate the filesystem, so both resolve their path through the single
//! containment check and both refuse to escape the workspace. `edit` is exact
//! string replacement: no regular expressions, so a replacement can never change
//! more than it was told to.

use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput};

use crate::domain::{args, schema};
use crate::exec::common::{as_output, read_text, workspace_path, write_text};

/// `write`.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteTool;

#[async_trait::async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write"
    }

    fn schema(&self) -> &'static serde_json::Value {
        schema::write()
    }

    async fn execute(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        Ok(as_output(write(ctx, raw)))
    }
}

fn write(ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
    let parsed: args::WriteArgs = args::parse("write", &raw)?;
    let path = workspace_path(ctx, Some(&parsed.path))?;
    // Read the previous content *before* overwriting: the delta is the model's
    // only cheap signal that this write undid a previous one (observed: an
    // hour-long loop rewriting the same 595 bytes with no visible progress).
    let before = std::fs::read_to_string(&path).ok();
    // UTF-8, never a BOM (design §6.1). The helper also creates parents and
    // strips a BOM the model may have echoed into `content`.
    write_text(&path, &parsed.content)?;
    let delta = before
        .filter(|before| before.as_str() != parsed.content)
        .map(|before| diff_summary(&before, &parsed.content));
    let mut receipt = format!(
        "wrote {} bytes to {}",
        ark_common::utf8_bytes_no_bom(&parsed.content).len(),
        path.display()
    );
    if let Some(delta) = delta {
        receipt.push_str(&format!("; {delta}"));
    }
    Ok(ToolOutput::ok(receipt))
}

/// One-line line-count delta between the previous and the new file content.
///
/// Line counts (not bytes) because edits move whole lines; the exact content
/// is already in the model's context. Returns a short "+a/-r" summary, or
/// `""` when the file did not exist before.
fn diff_summary(before: &str, after: &str) -> String {
    let before_lines = before.lines().count();
    let after_lines = after.lines().count();
    match after_lines.cmp(&before_lines) {
        std::cmp::Ordering::Equal => "line count unchanged".to_string(),
        std::cmp::Ordering::Greater => {
            format!("+{} lines", after_lines - before_lines)
        }
        std::cmp::Ordering::Less => {
            format!("-{} lines", before_lines - after_lines)
        }
    }
}

/// `edit`.
#[derive(Debug, Clone, Copy, Default)]
pub struct EditTool;

#[async_trait::async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn schema(&self) -> &'static serde_json::Value {
        schema::edit()
    }

    async fn execute(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        Ok(as_output(edit(ctx, raw)))
    }
}

fn edit(ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
    let parsed: args::EditArgs = args::parse("edit", &raw)?;
    if parsed.old_string.is_empty() {
        return Ok(ToolOutput::err("edit requires a non-empty old_string"));
    }
    let path = workspace_path(ctx, Some(&parsed.path))?;
    let original = read_text(&path)?;
    // Models emit `\n` in their arguments regardless of how the file is
    // stored, while this repository's working tree is CRLF (`core.autocrlf=true`
    // on Windows). A byte-exact match then fails on every multi-line anchor even
    // though the text is visibly there: 21 of one day's `edit` calls died on
    // "old_string not found" for that reason alone, and the model's only
    // recovery was to re-send the anchor with explicit `\r`. Normalize
    // the comparison to LF, and restore CRLF on write when the file was CRLF, so
    // a successful edit never rewrites the whole file's line endings.
    let crlf = original.contains("\r\n");
    let original_lf = to_lf(&original);
    let needle = to_lf(&parsed.old_string);
    let occurrences = original_lf.matches(&needle).count();
    if occurrences == 0 {
        return Ok(ToolOutput::err(format!(
            "old_string not found in {}",
            path.display()
        )));
    }
    let replace_all = parsed.replace_all.unwrap_or(false);
    if occurrences > 1 && !replace_all {
        return Ok(ToolOutput::err(format!(
            "old_string matches {occurrences} times in {}; pass replace_all=true or narrow the string",
            path.display()
        )));
    }
    // The replacement text arrives in the same LF-only shape, so it is joined
    // into the LF view and the file's own endings are restored afterwards.
    let replacement = to_lf(&parsed.new_string);
    let updated_lf = if replace_all {
        original_lf.replace(&needle, &replacement)
    } else {
        original_lf.replacen(&needle, &replacement, 1)
    };
    let updated = if crlf {
        to_crlf(&updated_lf)
    } else {
        updated_lf
    };
    std::fs::write(&path, updated.as_bytes())?;
    // A bare "replaced N occurrence(s)" gave the model no signal about *what*
    // changed*, so an edit that undid its own previous edit looked identical
    // to progress. The delta summary makes oscillation visible in the receipt.
    let delta = diff_summary(&original, &updated);
    let mut receipt = format!("replaced {occurrences} occurrence(s) in {}", path.display());
    if !delta.is_empty() {
        receipt.push_str(&format!("; {delta}"));
    }
    Ok(ToolOutput::ok(receipt))
}

/// Normalize every line ending to LF for anchor matching.
fn to_lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Restore CRLF line endings on an LF-normalized text.
fn to_crlf(text: &str) -> String {
    text.replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::testsupport::{ctx, temp_workspace};

    #[tokio::test]
    async fn write_creates_directories_and_content() {
        let dir = temp_workspace("write-basic");
        let raw = ToolArgs(serde_json::json!({ "path": "nested/a.txt", "content": "hi" }));
        let out = WriteTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok, "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.join("nested/a.txt")).unwrap(),
            "hi"
        );
    }

    #[tokio::test]
    async fn write_receipt_reports_the_line_delta_on_overwrite() {
        let dir = temp_workspace("write-delta");
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        let raw = ToolArgs(serde_json::json!({ "path": "a.txt", "content": "one\n" }));
        let out = WriteTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok, "{}", out.content);
        // The delta is the model's only cheap signal that this write undid a
        // previous one; it must be in the receipt, not just the byte count.
        assert!(out.content.contains("-1 lines"), "{}", out.content);
    }

    #[tokio::test]
    async fn edit_receipt_reports_the_line_delta() {
        let dir = temp_workspace("edit-delta");
        std::fs::write(dir.join("a.txt"), "a\nb\nc\n").unwrap();
        let raw = ToolArgs(serde_json::json!({
            "path": "a.txt",
            "old_string": "b\nc",
            "new_string": "x"
        }));
        let out = EditTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok, "{}", out.content);
        assert!(
            out.content.contains("replaced 1 occurrence(s)"),
            "{}",
            out.content
        );
        assert!(out.content.contains("-1 lines"), "{}", out.content);
    }

    #[tokio::test]
    async fn a_missing_content_does_not_silently_truncate_the_file() {
        let dir = temp_workspace("write-missing-content");
        let target = dir.join("a.txt");
        std::fs::write(&target, "keep").unwrap();
        for raw in [
            serde_json::json!({ "path": "a.txt" }),
            serde_json::json!({ "path": "a.txt", "content": null }),
        ] {
            let out = WriteTool.execute(&ctx(&dir), ToolArgs(raw)).await.unwrap();
            assert!(!out.ok, "content is required by the write schema");
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        }
    }

    #[tokio::test]
    async fn write_refuses_an_escape() {
        let dir = temp_workspace("write-escape");
        let raw = ToolArgs(serde_json::json!({ "path": "../out.txt", "content": "x" }));
        let out = WriteTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn edit_replaces_one_occurrence_by_default() {
        let dir = temp_workspace("edit-one");
        std::fs::write(dir.join("a.txt"), "aa").unwrap();
        let raw = ToolArgs(serde_json::json!({
            "path": "a.txt", "old_string": "a", "new_string": "b"
        }));
        let out = EditTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(!out.ok, "ambiguous single replace must refuse");
        assert!(out.content.contains("matches 2 times"));
    }

    #[tokio::test]
    async fn edit_replace_all_rewrites_every_occurrence() {
        let dir = temp_workspace("edit-all");
        std::fs::write(dir.join("a.txt"), "aa").unwrap();
        let raw = ToolArgs(serde_json::json!({
            "path": "a.txt", "old_string": "a", "new_string": "b", "replace_all": true
        }));
        let out = EditTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok, "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "bb");
    }

    #[tokio::test]
    async fn an_lf_anchor_matches_a_crlf_file() {
        // A model sends `\n` between anchor lines; a Windows checkout
        // stores `\r\n`. Byte-exact matching refused every such edit.
        let dir = temp_workspace("edit-crlf-anchor");
        let target = dir.join("a.txt");
        std::fs::write(&target, "one\r\ntwo\r\nthree\r\n").unwrap();
        let raw = ToolArgs(serde_json::json!({
            "path": "a.txt",
            "old_string": "one\ntwo",
            "new_string": "ONE\nTWO"
        }));
        let out = EditTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok, "{}", out.content);
        // The file keeps its own CRLF endings; only the matched text changed.
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "ONE\r\nTWO\r\nthree\r\n"
        );
    }

    #[tokio::test]
    async fn an_lf_file_stays_lf_after_an_edit() {
        let dir = temp_workspace("edit-lf-stays-lf");
        let target = dir.join("a.txt");
        std::fs::write(&target, "one\ntwo\n").unwrap();
        let raw = ToolArgs(serde_json::json!({
            "path": "a.txt",
            "old_string": "one\ntwo",
            "new_string": "ONE\nTWO"
        }));
        let out = EditTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok, "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "ONE\nTWO\n",
            "an LF file must not gain CRLF endings"
        );
    }

    #[tokio::test]
    async fn a_crlf_anchor_also_matches_a_crlf_file() {
        // The model may echo the file's real endings back; that must not be
        // counted as a second, different needle.
        let dir = temp_workspace("edit-crlf-anchor-native");
        let target = dir.join("a.txt");
        std::fs::write(&target, "one\r\ntwo\r\n").unwrap();
        let raw = ToolArgs(serde_json::json!({
            "path": "a.txt",
            "old_string": "one\r\ntwo",
            "new_string": "ONE\r\nTWO"
        }));
        let out = EditTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(out.ok, "{}", out.content);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "ONE\r\nTWO\r\n");
    }

    #[tokio::test]
    async fn edit_reports_a_missing_needle() {
        let dir = temp_workspace("edit-missing");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let raw = ToolArgs(serde_json::json!({
            "path": "a.txt", "old_string": "zzz", "new_string": "y"
        }));
        let out = EditTool.execute(&ctx(&dir), raw).await.unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("not found"));
    }
}
