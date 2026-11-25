//! Builtin `grep` tool (design §7.3).
//!
//! Content search without a regex engine dependency: the default is a plain
//! substring scan (case-sensitive unless `ignore_case`), which is what the
//! agent actually needs and what keeps the crate free of C-toolchain build
//! steps. A pattern carrying regex metacharacters is refused with a clear
//! error instead of being silently matched literally — session 936001de lost
//! 686 s to `0\.8|80%|…` returning a false "no matches".

use ark_ports::{Tool, ToolArgs, ToolConcurrency, ToolContext, ToolOutput};
use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::domain::{args, glob, schema};
use crate::exec::common::{
    as_output, is_ignored_dir_name, workspace_path, BlockingScan, ScanDeadline,
};
use crate::policy::{cap_all, DEFAULT_LIMITS};

/// Default matching lines returned.
pub const DEFAULT_GREP_LIMIT: usize = 200;
/// Files larger than this are skipped rather than scanned.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Longest emitted line. A minified bundle is one 254k-character line and the
/// line-count cap cannot see it; this bound is what actually protects the
/// model context (session 936001de pulled a whole `web/dist` chunk in).
const MAX_LINE_CHARS: usize = 400;
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
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::ReadOnly
    }

    async fn execute(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        Ok(as_output(run(ctx, raw).await))
    }
}

struct Query {
    needle: String,
    ignore_case: bool,
    file_glob: Option<String>,
    limit: usize,
    context: usize,
    /// Matching rows consumed across every file in this one scan.
    matched: Cell<usize>,
}

impl Query {
    /// `limit` counts *matching* lines, but every match may emit `2*context+1`
    /// lines. Cap the total output rows accordingly so context is not mistaken
    /// for a match (T-07) and a huge `context` cannot overflow (T-09).
    fn line_cap(&self) -> usize {
        self.limit
            .saturating_mul(self.context.saturating_mul(2).saturating_add(1))
    }

    fn budget_reached(&self) -> bool {
        self.matched.get() >= self.limit
    }
}

/// Regex metacharacters that would silently change meaning if matched
/// literally. ERE syntax, which is what `grep -E` and most models mean.
const REGEX_METACHARS: [char; 14] = [
    '.', '*', '+', '?', '(', ')', '[', ']', '{', '}', '|', '^', '$', '\\',
];

/// What a refused regex-looking pattern is told, in one place so the wording a
/// model reads names the escape hatch the tool actually accepts.
const REGEX_REFUSAL_MESSAGE: &str = concat!(
    "grep matches plain text only, and this pattern contains regex metacharacters ",
    "(. * + ? ( ) [ ] { } | ^ $ and backslash). Pass `literal: true` to search ",
    "for this exact text, or use `bash` with `grep -E` for a real regex.",
);

/// Refuse regex-looking patterns: a literal scan of `a|b` searches for the
/// three-character sequence and reports "no matches", which reads as a fact
/// about the codebase rather than about the pattern.
fn looks_like_regex(pattern: &str) -> bool {
    pattern.chars().any(|c| REGEX_METACHARS.contains(&c))
}

async fn run(ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
    let parsed: args::GrepArgs = args::parse("grep", &raw)?;
    let needle = parsed.pattern.clone().unwrap_or_default();
    if needle.is_empty() {
        return Ok(ToolOutput::err("grep requires a non-empty pattern"));
    }
    // `literal: true` is the documented opt-in for a needle that happens to
    // contain metacharacters (`getExecType()`, `syncFromTemplateBatch(`): the
    // caller already said it wants a byte-exact match, so refusing it makes the
    // flag useless and costs a round trip. Ignoring the flag was the observed
    // defect: 76 of one day's 347 `grep` calls were refused although 22 of them
    // had passed `literal: true`.
    let literal = parsed.literal.unwrap_or(false);
    if !literal && looks_like_regex(&needle) {
        return Ok(ToolOutput::err(REGEX_REFUSAL_MESSAGE));
    }
    let query = Query {
        needle,
        ignore_case: parsed.ignore_case.unwrap_or(false),
        file_glob: parsed.glob.clone(),
        limit: parsed.limit.unwrap_or(DEFAULT_GREP_LIMIT).max(1),
        context: parsed.context.unwrap_or(0),
        matched: Cell::new(0),
    };
    let empty_notice = query.needle.clone();
    #[cfg(windows)]
    if parsed
        .path
        .as_deref()
        .is_some_and(|path| is_windows_named_pipe_path(Path::new(path)))
    {
        return Ok(ToolOutput::ok(format!("no matches for `{empty_notice}`")));
    }
    let target = workspace_path(ctx, parsed.path.as_deref())?;
    let scan = BlockingScan::start();
    let line_cap = query.line_cap();
    let hits = scan
        .run(move |deadline| {
            let mut hits: Vec<String> = Vec::new();
            #[cfg(windows)]
            if is_windows_named_pipe_path(&target) {
                return Ok(Vec::new());
            }
            if target.is_file() {
                scan_file(&target, &target, &query, deadline, &mut hits);
            } else {
                let mut visited = HashSet::new();
                walk(
                    &target,
                    &target,
                    &query,
                    0,
                    deadline,
                    &mut hits,
                    &mut visited,
                );
            }
            deadline.check()?;
            Ok(hits)
        })
        .await?;
    if hits.is_empty() {
        return Ok(ToolOutput::ok(format!("no matches for `{empty_notice}`")));
    }
    let body = cap_all(&hits.join("\n"), &DEFAULT_LIMITS, Some(line_cap));
    Ok(ToolOutput::ok(body))
}

fn walk(
    root: &Path,
    dir: &Path,
    query: &Query,
    depth: usize,
    deadline: &ScanDeadline,
    out: &mut Vec<String>,
    visited: &mut HashSet<PathBuf>,
) {
    if depth > 32
        || out.len() >= query.line_cap()
        || query.budget_reached()
        || deadline.is_expired()
    {
        return;
    }
    // Canonicalize once per directory and never revisit it: a symlink cycle
    // otherwise re-expands the same tree at every level (T-02).
    let Ok(real_dir) = std::fs::canonicalize(dir) else {
        return;
    };
    if !visited.insert(real_dir) {
        return;
    }
    let Ok(real_root) = std::fs::canonicalize(root) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= query.line_cap() || query.budget_reached() || deadline.is_expired() {
            return;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // Never follow symlinks: a link out of the workspace would leak
        // external file contents, and a self-link would spin the walk.
        if kind.is_symlink() {
            continue;
        }
        // Build outputs and dependency caches are both noise (a bundle hit
        // flooded the context once) and volume (they made one grep walk
        // ~380k files). The name check is on every level below the search
        // root, so pointing `path` at one of these directories still works.
        if kind.is_dir() && is_ignored_dir_name(entry.file_name().to_string_lossy().as_ref()) {
            continue;
        }
        let path = entry.path();
        let Ok(real_path) = std::fs::canonicalize(&path) else {
            continue;
        };
        if !real_path.starts_with(&real_root) {
            continue;
        }
        if kind.is_dir() {
            walk(root, &path, query, depth + 1, deadline, out, visited);
        } else {
            scan_file(root, &path, query, deadline, out);
        }
    }
}

#[cfg(windows)]
fn is_windows_named_pipe_path(file: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;

    fn ascii_lower(value: u16) -> u16 {
        if (b'A' as u16..=b'Z' as u16).contains(&value) {
            value + 32
        } else {
            value
        }
    }

    fn has_prefix(file: &Path, device: u16) -> bool {
        let expected = [
            b'\\' as u16,
            b'\\' as u16,
            device,
            b'\\' as u16,
            b'p' as u16,
            b'i' as u16,
            b'p' as u16,
            b'e' as u16,
            b'\\' as u16,
        ];
        let mut actual = file.as_os_str().encode_wide();
        expected.into_iter().all(|want| {
            actual
                .next()
                .is_some_and(|got| ascii_lower(got) == ascii_lower(want))
        })
    }

    has_prefix(file, b'.' as u16) || has_prefix(file, b'?' as u16)
}

fn is_scannable_regular_file(file: &Path) -> bool {
    #[cfg(windows)]
    {
        // Even a metadata lookup can connect to a Windows named pipe. Reject
        // its reserved path before touching the filesystem.
        if is_windows_named_pipe_path(file) {
            return false;
        }
    }
    let Ok(meta) = std::fs::metadata(file) else {
        return false;
    };
    meta.is_file() && meta.len() <= MAX_FILE_BYTES
}

fn scan_file(
    root: &Path,
    file: &Path,
    query: &Query,
    deadline: &ScanDeadline,
    out: &mut Vec<String>,
) {
    if out.len() >= query.line_cap() || query.budget_reached() || deadline.is_expired() {
        return;
    }
    if !is_scannable_regular_file(file) {
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
    // Same encoding policy as `read`/`edit`: strict UTF-8 first, then the
    // documented Windows fallbacks. A GBK-encoded file would otherwise look
    // empty to `grep`, which silently hides real matches.
    let text = match ark_common::read_text(file) {
        Ok(text) => text,
        Err(_) => return,
    };
    let lines: Vec<&str> = text.lines().collect();
    // `ignore_case` folds only ASCII in the hot path: per-byte ASCII folding is
    // length- and char-boundary-preserving, so a byte offset found on the folded
    // line maps 1:1 onto the original line. Full Unicode `to_lowercase` can
    // change byte length (ẞ U+1E9E → ß, İ U+0130 → i + combining dot), which
    // silently shifted `hit_at` off its match and made `cut_line` cut a long
    // line around the wrong column — offsets must be measured on the line
    // `cut_line` actually slices. Lines searched under full folding therefore
    // remap the folded byte offset onto the original line instead.
    let ascii_fold = query.needle.is_ascii();
    let lowered_needle = query.needle.to_lowercase();
    let ascii_needle = ascii_fold_str(&query.needle);
    // Mark every line to emit exactly once: ':' for a real match, '-' for
    // context. Adjacent context windows overlap, so a single marker vector
    // deduplicates them while keeping the match marker authoritative. `limit`
    // counts matches, not output rows (T-07); all arithmetic is saturating so
    // a model-supplied `usize::MAX` cannot panic (T-09). The char offset of
    // each match is kept alongside so a truncated long line can be cut
    // around its match instead of a fixed prefix.
    let mut emit: Vec<Option<char>> = vec![None; lines.len()];
    let mut hit_at: Vec<usize> = vec![0; lines.len()];
    let needle_chars = query.needle.chars().count();
    for (index, line) in lines.iter().enumerate() {
        if query.matched.get() >= query.limit {
            break;
        }
        // Locate on folded text, but anchor on the original line: `hit_at`
        // indexes `lines[index]`, which `cut_line` slices.
        let hit_char = if !query.ignore_case {
            line.find(&query.needle)
                .map(|byte_at| line[..byte_at].chars().count())
        } else if ascii_fold {
            ascii_fold_str(line)
                .find(&ascii_needle)
                .map(|byte_at| line[..byte_at].chars().count())
        } else {
            line.to_lowercase()
                .find(&lowered_needle)
                .map(|folded_at| original_char_offset(line, folded_at))
        };
        let Some(hit_char) = hit_char else {
            continue;
        };
        query.matched.set(query.matched.get() + 1);
        hit_at[index] = hit_char;
        let start = index.saturating_sub(query.context);
        let end = index
            .saturating_add(query.context)
            .saturating_add(1)
            .min(lines.len());
        for slot in &mut emit[start..end] {
            if slot.is_none() {
                *slot = Some('-');
            }
        }
        emit[index] = Some(':');
    }
    let line_cap = query.line_cap();
    for (index, marker) in emit.into_iter().enumerate() {
        if out.len() >= line_cap {
            break;
        }
        if let Some(m) = marker {
            // Bound each emitted line around its match: minified bundles are
            // single-line files, and a fixed prefix would cut the match out
            // of anything past the first 400 columns.
            let line = cut_line(lines[index], hit_at[index], needle_chars);
            out.push(format!(
                "{relative}{m}{}{m}{line}",
                index + 1,
                m = m,
                line = line
            ));
        }
    }
}

/// Trim one output line to at most [`MAX_LINE_CHARS`] chars, keeping the
/// window that contains `hit_char` (the match position in chars) visible.
/// Centering on the match matters: a fixed prefix would cut the needle out of
/// every minified line past column 400, hiding what matched.
fn cut_line(line: &str, hit_char: usize, needle_chars: usize) -> String {
    let total = line.chars().count();
    if total <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let lead = MAX_LINE_CHARS.saturating_sub(needle_chars) / 2;
    let start = hit_char.saturating_sub(lead);
    let end = (start + MAX_LINE_CHARS).min(total);
    let start_byte = char_to_byte(line, start);
    let end_byte = char_to_byte(line, end);
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&line[start_byte..end_byte]);
    if end < total {
        out.push('…');
    }
    out
}

/// Byte offset of the `index`-th char; `line.len()` past the end.
fn char_to_byte(line: &str, index: usize) -> usize {
    line.char_indices()
        .nth(index)
        .map(|(byte, _)| byte)
        .unwrap_or(line.len())
}

/// Lowercase ASCII bytes in place; other bytes pass through untouched.
///
/// Used by the `ignore_case` fast path: unlike full `to_lowercase`, ASCII
/// folding never changes byte length or char boundaries, so byte offsets found
/// on the folded text stay valid on the original.
fn ascii_fold_str(text: &str) -> String {
    let mut bytes = text.as_bytes().to_vec();
    bytes.make_ascii_lowercase();
    // Per-byte ASCII folding cannot split a UTF-8 sequence (all folded bytes
    // stay < 0x80), so the result is still valid UTF-8.
    String::from_utf8(bytes).expect("ASCII folding keeps UTF-8 validity")
}

/// Map a char offset in `line.to_lowercase()` back onto `line`.
///
/// Full Unicode folding can change byte lengths (ẞ U+1E9E → ß, İ U+0130 →
/// i + combining dot), so folded text is not a byte image of the original.
/// Walking both texts in lockstep by char keeps the mapping exact: emit one
/// original char per folded char, and repeat original chars that folded into
/// several (İ → i + U+0307 needs one original char for two folded chars).
fn original_char_offset(line: &str, folded_at: usize) -> usize {
    let folded = line.to_lowercase();
    let mut folded_chars = folded[..folded_at].chars().count();
    let mut original_chars = 0usize;
    for original in line.chars() {
        if folded_chars == 0 {
            break;
        }
        let produced = original.to_lowercase().count();
        folded_chars = folded_chars.saturating_sub(produced);
        original_chars += 1;
    }
    original_chars
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
    async fn match_limit_is_shared_across_files() {
        let dir = temp_workspace("grep-global-limit");
        std::fs::write(dir.join("a.txt"), "needle\nneedle\nneedle\n").unwrap();
        std::fs::write(dir.join("b.txt"), "needle\nneedle\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "needle", "limit": 2, "context": 1 })),
            )
            .await
            .unwrap();
        assert!(out.ok, "{}", out.content);
        assert_eq!(
            out.content
                .lines()
                .filter(|line| line.contains(":needle"))
                .count(),
            2,
            "limit must bound matches across the whole walk: {}",
            out.content
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn reached_match_budget_skips_later_named_pipe_and_keeps_context() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::io::AsyncWriteExt;
        use tokio::net::windows::named_pipe::ServerOptions;

        let dir = temp_workspace("grep-budget-pipe");
        let first = dir.join("a.txt");
        std::fs::write(&first, "before\nneedle\nneedle\nafter\n").unwrap();
        let query = Query {
            needle: "needle".into(),
            ignore_case: false,
            file_glob: None,
            limit: 2,
            context: 10,
            matched: Cell::new(0),
        };
        let expected = vec![
            "a.txt-1-before".to_string(),
            "a.txt:2:needle".to_string(),
            "a.txt:3:needle".to_string(),
            "a.txt-4-after".to_string(),
        ];

        let pipe_name = format!(r"\\.\pipe\ark-grep-budget-{}", std::process::id());
        let mut pipe = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();
        let connected = Arc::new(AtomicBool::new(false));
        let server_connected = Arc::clone(&connected);
        let server = tokio::spawn(async move {
            if tokio::time::timeout(Duration::from_millis(200), pipe.connect())
                .await
                .is_ok_and(|result| result.is_ok())
            {
                server_connected.store(true, Ordering::SeqCst);
                let _ = pipe.write_all(b"needle\n").await;
            }
        });
        let scan = BlockingScan::start();
        let hits = tokio::time::timeout(
            Duration::from_secs(2),
            scan.run(move |deadline| {
                let mut hits = Vec::new();
                scan_file(&dir, &first, &query, deadline, &mut hits);
                assert_eq!(query.matched.get(), 2);
                assert_eq!(hits, expected, "current-file context must be complete");

                scan_file(&dir, Path::new(&pipe_name), &query, deadline, &mut hits);
                assert_eq!(hits, expected, "existing results must be returned intact");
                Ok(hits)
            }),
        )
        .await
        .expect("a satisfied match budget must not block on a later file")
        .unwrap();
        server.await.unwrap();

        assert!(
            !connected.load(Ordering::SeqCst),
            "a later file must not be opened after the global match budget is reached"
        );
        assert_eq!(hits.len(), 4);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn skips_non_regular_named_pipe_before_open() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::io::AsyncWriteExt;
        use tokio::net::windows::named_pipe::ServerOptions;

        let dir = temp_workspace("grep-non-regular-pipe");
        let query = Query {
            needle: "needle".into(),
            ignore_case: false,
            file_glob: None,
            limit: 2,
            context: 0,
            matched: Cell::new(0),
        };
        let pipe_name = format!(r"\\.\pipe\ark-grep-skip-{}", std::process::id());
        let mut pipe = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();
        let connected = Arc::new(AtomicBool::new(false));
        let server_connected = Arc::clone(&connected);
        let server = tokio::spawn(async move {
            if tokio::time::timeout(Duration::from_millis(200), pipe.connect())
                .await
                .is_ok_and(|result| result.is_ok())
            {
                server_connected.store(true, Ordering::SeqCst);
                let _ = pipe.write_all(b"needle\n").await;
            }
        });

        let scan = BlockingScan::start();
        let hits = tokio::time::timeout(
            Duration::from_secs(2),
            scan.run(move |deadline| {
                let mut hits = Vec::new();
                scan_file(&dir, Path::new(&pipe_name), &query, deadline, &mut hits);
                Ok(hits)
            }),
        )
        .await
        .expect("a non-regular file must not block the scan")
        .unwrap();
        server.await.unwrap();

        assert!(
            !connected.load(Ordering::SeqCst),
            "grep must skip a non-regular file before opening it"
        );
        assert!(hits.is_empty());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn explicit_named_pipe_path_is_skipped_before_top_level_metadata() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::io::AsyncWriteExt;
        use tokio::net::windows::named_pipe::ServerOptions;

        let dir = temp_workspace("grep-explicit-pipe");
        let mut context = ctx(&dir);
        context.sandbox = ark_ports::SandboxDecision::Unrestricted;
        let pipe_name = format!(r"\\.\pipe\ark-grep-explicit-{}", std::process::id());
        let mut pipe = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();
        let connected = Arc::new(AtomicBool::new(false));
        let server_connected = Arc::clone(&connected);
        let server = tokio::spawn(async move {
            if tokio::time::timeout(Duration::from_millis(200), pipe.connect())
                .await
                .is_ok_and(|result| result.is_ok())
            {
                server_connected.store(true, Ordering::SeqCst);
                let _ = pipe.write_all(b"needle\n").await;
            }
        });

        let output = tokio::time::timeout(
            Duration::from_secs(2),
            run(
                &context,
                ToolArgs(serde_json::json!({ "pattern": "needle", "path": pipe_name })),
            ),
        )
        .await
        .expect("an explicit named pipe must not block the scan")
        .unwrap();
        server.await.unwrap();

        assert!(
            !connected.load(Ordering::SeqCst),
            "grep must reject an explicit named pipe before its top-level metadata lookup"
        );
        assert!(output.ok);
        assert!(output.content.contains("no matches"), "{}", output.content);
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

    #[tokio::test]
    async fn regex_looking_patterns_are_refused_not_matched_literally() {
        let dir = temp_workspace("grep-regex-refused");
        // The literal sequence `a.c` does not occur; a literal scan would
        // return "no matches", which once read as a fact about the code.
        std::fs::write(dir.join("a.txt"), "abc\na.c\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "a.c" })),
            )
            .await
            .unwrap();
        assert!(!out.ok, "got {}", out.content);
        assert!(
            out.content.contains("metacharacters"),
            "got {}",
            out.content
        );
    }

    #[tokio::test]
    async fn literal_true_searches_needles_that_look_like_regex() {
        // The flag exists exactly for this case. Ignoring it meant a model that
        // already opted into a byte-exact search still had to rewrite the call
        // through `bash` (22 such refusals observed in one day).
        let dir = temp_workspace("grep-literal-accepted");
        std::fs::write(dir.join("a.java"), "int x = getExecType();\n").unwrap();
        for pattern in ["getExecType()", "ExecType()|Other("] {
            let out = GrepTool
                .execute(
                    &ctx(&dir),
                    ToolArgs(serde_json::json!({ "pattern": pattern, "literal": true })),
                )
                .await
                .unwrap();
            assert!(
                out.ok,
                "literal search must not be refused: {}",
                out.content
            );
            if pattern == "getExecType()" {
                assert!(out.content.contains("a.java:1:"), "got {}", out.content);
            } else {
                assert!(out.content.contains("no matches"), "got {}", out.content);
            }
        }
    }

    #[tokio::test]
    async fn a_regex_refusal_names_the_literal_flag() {
        let dir = temp_workspace("grep-refusal-message");
        std::fs::write(dir.join("a.txt"), "x\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "a.*b" })),
            )
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(
            out.content.contains("literal: true"),
            "the refusal must name the escape hatch: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn alternation_patterns_are_refused() {
        let dir = temp_workspace("grep-regex-alt");
        std::fs::write(dir.join("a.txt"), "threshold\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({
                    "pattern": "0\\.8|80%|threshold.*0\\.8"
                })),
            )
            .await
            .unwrap();
        assert!(!out.ok, "got {}", out.content);
    }

    #[tokio::test]
    async fn build_and_cache_directories_are_skipped() {
        let dir = temp_workspace("grep-ignore-dirs");
        for name in [
            "target",
            "node_modules",
            "dist",
            ".git",
            ".tmp",
            ".worktrees",
        ] {
            let sub = dir.join(name);
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join("gen.js"), "needle-in-generated\n").unwrap();
        }
        std::fs::write(dir.join("src.txt"), "needle-in-source\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "needle" })),
            )
            .await
            .unwrap();
        assert!(out.content.contains("src.txt"), "got {}", out.content);
        assert!(
            !out.content.contains("needle-in-generated"),
            "generated output must not be scanned: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn an_ignored_directory_can_still_be_searched_explicitly() {
        let dir = temp_workspace("grep-ignore-explicit");
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target/gen.js"), "needle\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "needle", "path": "target" })),
            )
            .await
            .unwrap();
        // The relative prefix is relative to the explicit search root, so the
        // file shows as `gen.js`; the point is that the ignored directory was
        // still searched instead of silently yielding nothing.
        assert_eq!(out.content, "gen.js:1:needle\n");
    }

    #[tokio::test]
    async fn ignore_case_keeps_the_cut_window_on_the_match_when_folding_expands_chars() {
        let dir = temp_workspace("grep-fold-offset");
        // Case 1: length-changing chars in the LINE, ASCII needle — the
        // per-byte ASCII fast path must anchor the offset on the original
        // line instead of the folded one.
        let line = format!("{}dotted{}", "İ".repeat(300), "z".repeat(500));
        std::fs::write(dir.join("fold.txt"), &line).unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "DOTTED", "ignore_case": true })),
            )
            .await
            .unwrap();
        assert!(out.ok, "{}", out.content);
        assert!(
            out.content.contains("dotted"),
            "the cut window must still contain the match: {}",
            out.content
        );

        // Case 2: length-changing chars in the NEEDLE — full folding runs,
        // and its folded char offset must be remapped onto the original line.
        let line = format!("{}İdotted{}", "İ".repeat(300), "z".repeat(500));
        std::fs::write(dir.join("folded-needle.txt"), &line).unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "İDOTTED", "ignore_case": true })),
            )
            .await
            .unwrap();
        assert!(out.ok, "{}", out.content);
        assert!(
            out.content.contains("dotted"),
            "the cut window must still contain the match: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn ignore_case_ascii_fast_path_still_matches_case_variants() {
        let dir = temp_workspace("grep-ascii-fold");
        std::fs::write(
            dir.join("a.txt"),
            "getExecType()\nGETEXECTYPE()\ngetexectype()\n",
        )
        .unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "GetExecType", "ignore_case": true })),
            )
            .await
            .unwrap();
        assert_eq!(out.content.lines().count(), 3, "{}", out.content);
        assert!(out.content.contains("a.txt:1:getExecType()"));
        assert!(out.content.contains("a.txt:2:GETEXECTYPE()"));
        assert!(out.content.contains("a.txt:3:getexectype()"));
    }

    #[tokio::test]
    async fn a_minified_single_line_match_is_cut_to_a_bounded_prefix() {
        let dir = temp_workspace("grep-line-cut");
        let bundle = format!("{}needle{}", "x".repeat(100_000), "y".repeat(100_000));
        std::fs::write(dir.join("bundle.js"), bundle).unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "needle" })),
            )
            .await
            .unwrap();
        assert!(out.ok);
        assert!(
            out.content.len() < 2_000,
            "one match line must stay bounded, got {} chars",
            out.content.len()
        );
        assert!(out.content.contains("needle"));
    }

    #[tokio::test]
    async fn overlapping_context_windows_emit_each_line_once() {
        let dir = temp_workspace("grep-context-overlap");
        std::fs::write(dir.join("a.txt"), "hit-a\nhit-b\nhit-c\nhit-d\nhit-e\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "hit", "context": 1 })),
            )
            .await
            .unwrap();
        for n in 1..=5 {
            let occurrences = out.content.matches(&format!("a.txt-{n}-")).count()
                + out.content.matches(&format!("a.txt:{n}:")).count();
            assert_eq!(
                occurrences, 1,
                "line {n} must appear exactly once: {}",
                out.content
            );
        }
    }

    #[tokio::test]
    async fn limit_counts_matches_not_context_rows() {
        let dir = temp_workspace("grep-limit-context");
        std::fs::write(dir.join("a.txt"), "m1\nm2\nm3\nm4\nm5\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "m", "limit": 2, "context": 2 })),
            )
            .await
            .unwrap();
        // Exactly two *matching* lines; later lines may still appear as
        // context rows (`-`), which is correct, but never as matches (`:`).
        assert!(out.content.contains("a.txt:1:m1"));
        assert!(out.content.contains("a.txt:2:m2"));
        assert!(
            !out.content.contains("a.txt:3:m3"),
            "a third real match must not be emitted: {}",
            out.content
        );
        assert!(!out.content.contains("a.txt:5:m5"));
    }

    #[tokio::test]
    async fn a_max_usize_context_does_not_panic() {
        let dir = temp_workspace("grep-overflow");
        std::fs::write(dir.join("a.txt"), "needle\n").unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "needle", "context": usize::MAX })),
            )
            .await
            .unwrap();
        assert!(out.ok);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn grep_does_not_follow_symlinks_out_of_the_workspace() {
        use std::os::unix::fs::symlink;
        let dir = temp_workspace("grep-symlink-escape");
        let outside = temp_workspace("grep-symlink-outside");
        std::fs::write(outside.join("secret.txt"), "needle-secret\n").unwrap();
        symlink(&outside, dir.join("link")).unwrap();
        let out = GrepTool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "pattern": "needle" })),
            )
            .await
            .unwrap();
        assert!(
            !out.content.contains("needle-secret"),
            "a symlink must not leak external contents: {}",
            out.content
        );
    }
}
