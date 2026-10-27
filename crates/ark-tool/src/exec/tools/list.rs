//! Builtin `ls` and `find` tools (design §7.3).
//!
//! Both are read-only directory walkers. `ls` reads one level, `find` recurses
//! with a glob filter; both stop at the configured limits and say so, so a huge
//! tree cannot blow up the model context.

use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput};
use std::path::Path;

use crate::domain::{args, glob, schema};
use crate::exec::common::{as_output, with_detail, workspace_path};
use crate::policy::{cap_all, DEFAULT_LIMITS};

/// Default entries returned by `ls`.
pub const DEFAULT_LS_LIMIT: usize = 500;
/// Default matches returned by `find`.
pub const DEFAULT_FIND_LIMIT: usize = 500;
/// Depth cap so a symlink cycle cannot spin forever.
pub const MAX_DEPTH: usize = 32;

/// `ls`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LsTool;

#[async_trait::async_trait]
impl Tool for LsTool {
    fn name(&self) -> &'static str {
        "ls"
    }

    fn schema(&self) -> &'static serde_json::Value {
        schema::ls()
    }

    async fn execute(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        Ok(as_output(run(ctx, raw)))
    }
}

fn run(ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
    let parsed: args::LsArgs = args::parse("ls", &raw)?;
    let dir = workspace_path(ctx, parsed.path.as_deref())?;
    let limit = parsed.limit.unwrap_or(DEFAULT_LS_LIMIT).max(1);
    let mut entries: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let marker = if entry.path().is_dir() { "/" } else { "" };
        entries.push(format!("{name}{marker}"));
    }
    entries.sort();
    let total = entries.len();
    let shown: Vec<String> = entries.into_iter().take(limit).collect();
    let body = cap_all(&shown.join("\n"), &DEFAULT_LIMITS, Some(limit));
    Ok(with_detail(
        ToolOutput::ok(body),
        format!(
            "{}: {}/{} entries",
            dir.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            shown.len().min(total),
            total
        ),
    ))
}

/// `find`.
#[derive(Debug, Clone, Copy, Default)]
pub struct FindTool;

#[async_trait::async_trait]
impl Tool for FindTool {
    fn name(&self) -> &'static str {
        "find"
    }

    fn schema(&self) -> &'static serde_json::Value {
        schema::find()
    }

    async fn execute(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        Ok(as_output(run_find(ctx, raw)))
    }
}

fn run_find(ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
    let parsed: args::FindArgs = args::parse("find", &raw)?;
    let pattern = parsed.pattern.clone().unwrap_or_else(|| "*".to_string());
    let root = workspace_path(ctx, parsed.path.as_deref())?;
    let limit = parsed.limit.unwrap_or(DEFAULT_FIND_LIMIT).max(1);
    let name_only = glob::is_name_pattern(&pattern);
    let mut found = Vec::new();
    walk(&root, &root, &pattern, name_only, limit, 0, &mut found);
    found.sort();
    let body = cap_all(&found.join("\n"), &DEFAULT_LIMITS, Some(limit));
    Ok(ToolOutput::ok(body))
}

fn walk(
    root: &Path,
    dir: &Path,
    pattern: &str,
    name_only: bool,
    limit: usize,
    depth: usize,
    out: &mut Vec<String>,
) {
    if depth > MAX_DEPTH || out.len() >= limit {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= limit {
            return;
        }
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map(|rest| rest.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let candidate = if name_only {
            entry.file_name().to_string_lossy().to_string()
        } else {
            relative.clone()
        };
        if !relative.is_empty() && glob::matches(pattern, &candidate) {
            out.push(relative);
        }
        if path.is_dir() {
            walk(root, &path, pattern, name_only, limit, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::testsupport::{ctx, temp_workspace};

    #[tokio::test]
    async fn ls_lists_one_level_sorted_with_dir_markers() {
        let dir = temp_workspace("ls-basic");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("b.txt"), "x").unwrap();
        let out = LsTool
            .execute(&ctx(&dir), ToolArgs(serde_json::json!({})))
            .await
            .unwrap();
        assert!(out.ok);
        assert!(out.content.contains("sub/"), "got {}", out.content);
        assert!(out.content.contains("b.txt"));
    }

    #[tokio::test]
    async fn find_matches_names_recursively() {
        let dir = temp_workspace("find-name");
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::write(dir.join("src/deep/main.rs"), "x").unwrap();
        std::fs::write(dir.join("src/other.txt"), "x").unwrap();
        let out = FindTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "*.rs" })),
            )
            .await
            .unwrap();
        assert!(out.ok);
        assert!(out.content.contains("main.rs"), "got {}", out.content);
        assert!(!out.content.contains("other.txt"));
    }

    #[tokio::test]
    async fn find_honours_the_limit() {
        let dir = temp_workspace("find-limit");
        for index in 0..5 {
            std::fs::write(dir.join(format!("f{index}.txt")), "x").unwrap();
        }
        let out = FindTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "*.txt", "limit": 2 })),
            )
            .await
            .unwrap();
        assert_eq!(out.content.lines().count(), 2);
    }

    #[tokio::test]
    async fn ls_refuses_an_escape() {
        let dir = temp_workspace("ls-escape");
        let out = LsTool
            .execute(&ctx(&dir), ToolArgs(serde_json::json!({ "path": "../.." })))
            .await
            .unwrap();
        assert!(!out.ok);
    }
}
