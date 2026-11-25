//! Builtin `ls` and `find` tools (design §7.3).
//!
//! Both are read-only directory walkers. `ls` reads one level, `find` recurses
//! with a glob filter; both stop at the configured limits and say so, so a huge
//! tree cannot blow up the model context. The recursion skips build/dependency
//! directories and runs on the blocking pool under the shared scan deadline,
//! mirroring `grep`.

use ark_ports::{Tool, ToolArgs, ToolConcurrency, ToolContext, ToolOutput};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::domain::{args, glob, schema};
use crate::exec::common::{
    as_output, entry_inside_root, is_ignored_dir_name, with_detail, workspace_path, BlockingScan,
    ScanDeadline,
};
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
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::ReadOnly
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
        // `Path::is_dir` follows symlinks; use the dir-entry type so a link
        // never masquerades as a real directory.
        let marker = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            "/"
        } else {
            ""
        };
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
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::ReadOnly
    }

    async fn execute(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        Ok(as_output(run_find(ctx, raw).await))
    }
}

async fn run_find(ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
    let parsed: args::FindArgs = args::parse("find", &raw)?;
    let pattern = parsed.pattern.clone().unwrap_or_else(|| "*".to_string());
    let root = workspace_path(ctx, parsed.path.as_deref())?;
    let limit = parsed.limit.unwrap_or(DEFAULT_FIND_LIMIT).max(1);
    let name_only = glob::is_name_pattern(&pattern);
    // The containment anchor is resolved once per scan; re-canonicalizing the
    // root at every recursion level was one redundant syscall chain per
    // directory.
    let real_root = match std::fs::canonicalize(&root) {
        Ok(real_root) => real_root,
        Err(_) => root.clone(),
    };
    let scan = BlockingScan::start();
    let mut found = scan
        .run(move |deadline| {
            let mut found = Vec::new();
            let mut visited = HashSet::new();
            walk(
                &real_root,
                &root,
                &root,
                &pattern,
                name_only,
                limit,
                0,
                deadline,
                &mut found,
                &mut visited,
            );
            deadline.check()?;
            Ok(found)
        })
        .await?;
    found.sort();
    let body = cap_all(&found.join("\n"), &DEFAULT_LIMITS, Some(limit));
    Ok(ToolOutput::ok(body))
}

/// Pre-existing lint gate precedent (`state.rs`): the recursive walker
/// carries ten parameters (containment anchor + display root joined the
/// signature); regrouping would touch every recursive call site for zero
/// behavior change, so `#[allow(clippy::too_many_arguments)]` applies.
#[allow(clippy::too_many_arguments)]
fn walk(
    real_root: &Path,
    display_root: &Path,
    dir: &Path,
    pattern: &str,
    name_only: bool,
    limit: usize,
    depth: usize,
    deadline: &ScanDeadline,
    out: &mut Vec<String>,
    visited: &mut HashSet<PathBuf>,
) {
    if depth > MAX_DEPTH || out.len() >= limit || deadline.is_expired() {
        return;
    }
    // Canonicalize once per directory and never revisit it: a symlink cycle
    // (or a bind mount) otherwise re-expands the same tree at every level.
    // The root's canonical image was resolved once by the caller.
    let Ok(real_dir) = std::fs::canonicalize(dir) else {
        return;
    };
    if !visited.insert(real_dir) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= limit || deadline.is_expired() {
            return;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // Never follow a symlink: a link out of the workspace would leak
        // external names, and a self-referencing link would spin the walk.
        if kind.is_symlink() {
            continue;
        }
        // Same skip list as `grep`: build outputs and caches are both volume
        // and noise. The name check is on every level below the search root,
        // so pointing `path` at one of these directories still works.
        if kind.is_dir() && is_ignored_dir_name(entry.file_name().to_string_lossy().as_ref()) {
            continue;
        }
        let path = entry.path();
        // Containment re-check — see `entry_inside_root` (design ark-tool.md:15).
        if !entry_inside_root(kind.is_dir(), &path, real_root) {
            continue;
        }
        let relative = path
            .strip_prefix(display_root)
            .or_else(|_| path.strip_prefix(real_root))
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
        if kind.is_dir() {
            walk(
                real_root,
                display_root,
                &path,
                pattern,
                name_only,
                limit,
                depth + 1,
                deadline,
                out,
                visited,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::common::IGNORED_DIR_NAMES;
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
    async fn find_skips_build_and_cache_directories() {
        let dir = temp_workspace("find-ignore-dirs");
        // Iterate the real constant: a name added later lands here or the
        // list change ships without coverage.
        for name in IGNORED_DIR_NAMES {
            let sub = dir.join(name);
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join("generated.rs"), "x").unwrap();
        }
        std::fs::write(dir.join("real.rs"), "x").unwrap();
        let out = FindTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "**/*.rs" })),
            )
            .await
            .unwrap();
        assert!(out.content.contains("real.rs"), "got {}", out.content);
        assert!(
            !out.content.contains("generated.rs"),
            "build/cache directories must not be walked: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn an_ignored_directory_can_still_be_searched_explicitly() {
        let dir = temp_workspace("find-ignore-explicit");
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target/generated.rs"), "x").unwrap();
        let out = FindTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "**/*.rs", "path": "target" })),
            )
            .await
            .unwrap();
        // Paths are relative to the explicit search root; the point is the
        // ignored directory was searched instead of silently yielding nothing.
        assert_eq!(out.content, "generated.rs\n");
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

    #[cfg(unix)]
    #[tokio::test]
    async fn find_does_not_follow_symlinks_out_of_the_workspace() {
        use std::os::unix::fs::symlink;
        let dir = temp_workspace("find-symlink-escape");
        let outside = temp_workspace("find-symlink-outside");
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        symlink(&outside, dir.join("link")).unwrap();
        let out = FindTool
            .execute(&ctx(&dir), ToolArgs(serde_json::json!({ "pattern": "*" })))
            .await
            .unwrap();
        assert!(
            !out.content.contains("secret.txt"),
            "a symlink must not leak external names: {}",
            out.content
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_cycles_do_not_duplicate_paths() {
        use std::os::unix::fs::symlink;
        let dir = temp_workspace("find-symlink-cycle");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::write(dir.join("a/keep.txt"), "x").unwrap();
        symlink("..", dir.join("a/back")).unwrap();
        let out = FindTool
            .execute(&ctx(&dir), ToolArgs(serde_json::json!({ "pattern": "*" })))
            .await
            .unwrap();
        let mut seen = std::collections::HashSet::new();
        for line in out.content.lines() {
            assert!(seen.insert(line), "duplicate path {line}");
        }
    }
}
