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
    // UTF-8, never a BOM (design §6.1). The helper also creates parents and
    // strips a BOM the model may have echoed into `content`.
    write_text(&path, &parsed.content)?;
    Ok(ToolOutput::ok(format!(
        "wrote {} bytes to {}",
        ark_common::utf8_bytes_no_bom(&parsed.content).len(),
        path.display()
    )))
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
    let occurrences = original.matches(&parsed.old_string).count();
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
    let updated = if replace_all {
        original.replace(&parsed.old_string, &parsed.new_string)
    } else {
        original.replacen(&parsed.old_string, &parsed.new_string, 1)
    };
    std::fs::write(&path, updated.as_bytes())?;
    Ok(ToolOutput::ok(format!(
        "replaced {occurrences} occurrence(s) in {}",
        path.display()
    )))
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
