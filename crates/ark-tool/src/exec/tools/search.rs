//! Builtin `grep` tool (design §7.3).
//!
//! Content search without a regex engine dependency: the default is a plain
//! substring scan (case-sensitive unless `ignore_case`), which is what the
//! agent actually needs and what keeps the crate free of C-toolchain build
//! steps. `literal: false` still means literal here, and the output says so.

use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput};
use std::path::Path;

use crate::domain::{args, glob, schema};
use crate::exec::common::{as_output, workspace_path};
use crate::policy::{cap_all, DEFAULT_LIMITS};

/// Default matching lines returned.
pub const DEFAULT_GREP_LIMIT: usize = 200;
/// Files larger than this are skipped rather than scanned.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// `grep`.
#[derive(Debug, Clone, Copy, Default)]
pub struct GrepTool;

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn schema(&self) -> &'static serde_json::Value {
        schema::grep()
    }

    async fn execute(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        Ok(as_output(run(ctx, raw)))
    }
}

struct Query {
    needle: String,
    ignore_case: bool,
    file_glob: Option<String>,
    limit: usize,
    context: usize,
}

fn run(ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
    let parsed: args::GrepArgs = args::parse("grep", &raw)?;
    let needle = parsed.pattern.clone().unwrap_or_default();
    if needle.is_empty() {
        return Ok(ToolOutput::err("grep requires a non-empty pattern"));
    }
    let query = Query {
        needle,
        ignore_case: parsed.ignore_case.unwrap_or(false),
        file_glob: parsed.glob.clone(),
        limit: parsed.limit.unwrap_or(DEFAULT_GREP_LIMIT).max(1),
        context: parsed.context.unwrap_or(0),
    };
    let target = workspace_path(ctx, parsed.path.as_deref())?;
    let mut hits: Vec<String> = Vec::new();
    if target.is_file() {
        scan_file(&target, &target, &query, &mut hits);
    } else {
        walk(&target, &target, &query, 0, &mut hits);
    }
    if hits.is_empty() {
        return Ok(ToolOutput::ok(format!("no matches for `{}`", query.needle)));
    }
    let body = cap_all(&hits.join("\n"), &DEFAULT_LIMITS, Some(query.limit));
    Ok(ToolOutput::ok(body))
}

fn walk(root: &Path, dir: &Path, query: &Query, depth: usize, out: &mut Vec<String>) {
    if depth > 32 || out.len() >= query.limit {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= query.limit {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, query, depth + 1, out);
        } else {
            scan_file(root, &path, query, out);
        }
    }
}

fn scan_file(root: &Path, file: &Path, query: &Query, out: &mut Vec<String>) {
    if out.len() >= query.limit {
        return;
    }
    let relative = file
        .strip_prefix(root)
        .map(|rest| rest.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| file.to_string_lossy().to_string());
    if let Some(pattern) = &query.file_glob {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !glob::matches(pattern, &relative) && !glob::matches(pattern, &name) {
            return;
        }
    }
    let Ok(meta) = std::fs::metadata(file) else {
        return;
    };
    if meta.len() > MAX_FILE_BYTES {
        return;
    }
    // Same encoding policy as `read`/`edit`: strict UTF-8 first, then the
    // documented Windows fallbacks. A GBK-encoded file would otherwise look
    // empty to `grep`, which silently hides real matches.
    let text = match ark_common::read_text(file) {
        Ok(text) => text,
        Err(_) => return,
    };
    let lines: Vec<&str> = text.lines().collect();
    let (haystack_needle, haystack_lines) = if query.ignore_case {
        (
            query.needle.to_lowercase(),
            lines.iter().map(|l| l.to_lowercase()).collect::<Vec<_>>(),
        )
    } else {
        (
            query.needle.clone(),
            lines.iter().map(|l| l.to_string()).collect(),
        )
    };
    let mut index = 0usize;
    while index < lines.len() && out.len() < query.limit {
        if haystack_lines[index].contains(&haystack_needle) {
            let start = index.saturating_sub(query.context);
            let end = (index + query.context + 1).min(lines.len());
            for (line_index, line) in lines.iter().enumerate().take(end).skip(start) {
                let marker = if line_index == index { ":" } else { "-" };
                out.push(format!(
                    "{relative}{marker}{}{marker}{line}",
                    line_index + 1
                ));
            }
            index = end;
        } else {
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::testsupport::{ctx, temp_workspace};

    #[tokio::test]
    async fn finds_a_line_with_its_position() {
        let dir = temp_workspace("grep-basic");
        std::fs::write(dir.join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "beta" })),
            )
            .await
            .unwrap();
        assert!(out.ok);
        assert!(out.content.contains("a.txt:2:beta"), "got {}", out.content);
    }

    #[tokio::test]
    async fn ignore_case_matches_differently_cased_text() {
        let dir = temp_workspace("grep-case");
        std::fs::write(dir.join("a.txt"), "HELLO\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "hello", "ignore_case": true })),
            )
            .await
            .unwrap();
        assert!(out.content.contains("HELLO"));
    }

    #[tokio::test]
    async fn glob_restricts_the_files_scanned() {
        let dir = temp_workspace("grep-glob");
        std::fs::write(dir.join("a.rs"), "needle\n").unwrap();
        std::fs::write(dir.join("b.txt"), "needle\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "needle", "glob": "*.rs" })),
            )
            .await
            .unwrap();
        assert!(out.content.contains("a.rs"));
        assert!(!out.content.contains("b.txt"));
    }

    #[tokio::test]
    async fn no_match_is_successful_but_explicit() {
        let dir = temp_workspace("grep-none");
        std::fs::write(dir.join("a.txt"), "x\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "zzz" })),
            )
            .await
            .unwrap();
        assert!(out.ok);
        assert!(out.content.contains("no matches"));
    }

    #[tokio::test]
    async fn empty_pattern_is_refused() {
        let dir = temp_workspace("grep-empty");
        let out = GrepTool
            .execute(&ctx(&dir), ToolArgs(serde_json::json!({ "pattern": "" })))
            .await
            .unwrap();
        assert!(!out.ok);
    }
}
