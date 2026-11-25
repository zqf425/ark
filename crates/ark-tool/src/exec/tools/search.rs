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
    as_output, entry_inside_root, is_ignored_dir_name, workspace_path, BlockingScan, ScanDeadline,
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
    // The containment anchor is resolved once per scan: re-canonicalizing the
    // root at every recursion level cost one full root resolution per
    // directory (800 dirs = 800 redundant syscalls on a cold cache).
    let real_root = match std::fs::canonicalize(&target) {
        Ok(real_root) => real_root,
        Err(_) => target.clone(),
    };
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
                    &real_root,
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

/// The walker carries the containment anchor, the display root and the live
/// budget together; regrouping would touch every recursive call site for zero
/// behavior change, so the established too-many-arguments precedent applies.
#[allow(clippy::too_many_arguments)]
fn walk(
    real_root: &Path,
    display_root: &Path,
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
    // otherwise re-expands the same tree at every level (T-02). The root's
    // canonical image was resolved once by the caller.
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
        // Containment re-check — see `common::entry_inside_root` for the
        // per-platform scoping and the dirent premise it rests on.
        if !entry_inside_root(kind.is_dir(), &path, real_root) {
            continue;
        }
        if kind.is_dir() {
            walk(
                real_root,
                display_root,
                &path,
                query,
                depth + 1,
                deadline,
                out,
                visited,
            );
        } else {
            let relative = path
                .strip_prefix(display_root)
                .or_else(|_| path.strip_prefix(real_root))
                .map(|rest| rest.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().to_string());
            scan_file_prefixed(&relative, &path, query, deadline, out);
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
    let relative = file
        .strip_prefix(root)
        .map(|rest| rest.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| file.to_string_lossy().to_string());
    scan_file_prefixed(&relative, file, query, deadline, out);
}

/// Whether a `--glob` pattern accepts this file: matched against the
/// relative path first, then the bare file name (master behavior: `*.rs`
/// must match `src/main.rs` via its name).
fn glob_matches_file(pattern: &str, relative: &str, file: &Path) -> bool {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    glob::matches(pattern, relative) || glob::matches(pattern, &name)
}

/// Scan one file whose output prefix the walker already computed. Keeping the
/// prefix separate lets the walker stay relative to the user-visible root
/// spelling (an explicit `path` into an ignored directory still shows
/// `gen.js`, not the absolute temp path).
fn scan_file_prefixed(
    relative: &str,
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
    if let Some(pattern) = &query.file_glob {
        if !glob_matches_file(pattern, relative, file) {
            return;
        }
    }
    // Same encoding policy as `read`/`edit`: strict UTF-8 first, then the
    // documented Windows fallbacks. A GBK-encoded file would otherwise look
    // empty to `grep`, which silently hides real matches.
    //
    // A decoded NUL marks object files, images and other binary blobs: the
    // needle would have to span raw object bytes to match, and one hit inside
    // them both floods the output and reads as a fact about source code. The
    // check runs on the decoded text (UTF-16LE without a BOM decodes with
    // interleaved NULs and is skipped too — `rg` applies the same heuristic).
    let text = match ark_common::read_text_file(file) {
        Ok(decoded) => {
            if decoded.text.contains('\0') {
                return;
            }
            decoded.text
        }
        Err(_) => return,
    };
    let lines: Vec<&str> = text.lines().collect();
    let needles = FoldedNeedle::new(query);
    // Mark every line to emit exactly once: ':' for a real match, '-' for
    // context. Adjacent context windows overlap, so a single marker vector
    // deduplicates them while keeping the match marker authoritative. `limit`
    // counts matches, not output rows (T-07); all arithmetic is saturating so
    // a model-supplied `usize::MAX` cannot panic (T-09). The char offset of
    // each match is kept alongside so a truncated long line can be cut
    // around its match instead of a fixed prefix.
    let mut emit: Vec<Option<char>> = vec![None; lines.len()];
    let mut hit_at: Vec<usize> = vec![0; lines.len()];
    for (index, line) in lines.iter().enumerate() {
        if query.matched.get() >= query.limit {
            break;
        }
        let Some(hit_char) = needles.locate(line) else {
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
    emit_rows(
        &emit,
        &lines,
        &hit_at,
        relative,
        query.needle.chars().count(),
        query.line_cap(),
        out,
    );
}

/// The needle in the two foldings a line is searched with.
///
/// `ignore_case` folds only ASCII in the hot path: ASCII folding never changes
/// byte length or char boundaries, so a byte offset found on the folded line
/// maps 1:1 onto the original line. Full Unicode `to_lowercase` can expand
/// chars (İ U+0130 → i + combining dot), which silently shifted `hit_at` off
/// its match and made `cut_line` cut a long line around the wrong column —
/// offsets must be measured on the line `cut_line` slices, so the
/// full-folding path remaps the folded byte offset onto the original line
/// instead. Trade-off, deliberate: the ASCII path misses matches through
/// non-ASCII chars that fold *into* ASCII (K U+212A → k, İ → i); the old
/// full-fold behavior found those but also matched whole Turkish lines when
/// searching `i`.
struct FoldedNeedle {
    exact: String,
    ignore_case: bool,
    needle_is_ascii: bool,
    ascii: String,
    lowered: String,
}

impl FoldedNeedle {
    fn new(query: &Query) -> Self {
        Self {
            exact: query.needle.clone(),
            ignore_case: query.ignore_case,
            needle_is_ascii: query.needle.is_ascii(),
            ascii: query.needle.to_ascii_lowercase(),
            lowered: query.needle.to_lowercase(),
        }
    }

    /// Char offset of the first match in `line`, anchored on the original
    /// line, or `None` when the line does not match.
    ///
    /// The decision uses `contains` (the SIMD substring kernel for needles up
    /// to 32 bytes), and only hit rows pay for the second, locating scan:
    /// hits are sparse relative to scanned lines. Longer needles gain nothing
    /// from `contains` — both it and `find` fall back to TwoWay, so a single
    /// `find` decides and locates in one pass. The decision must stay a plain
    /// OR of the two folds — an ASCII-folded hit IS a hit, and only rows it
    /// misses pay for the full fold.
    fn locate(&self, line: &str) -> Option<usize> {
        if !self.ignore_case {
            return line
                .find(&self.exact)
                .map(|byte_at| line[..byte_at].chars().count());
        }
        if self.exact.len() > 32 {
            // TwoWay for both primitives: one folded pass decides and
            // locates. The fold is mandatory — `find` on the raw line would
            // skip case folding entirely and turn every differently-cased
            // line into a false "no matches" (a P0 caught in review; the
            // regression below pins it).
            let folded = line.to_lowercase();
            return folded
                .find(&self.lowered)
                .map(|folded_byte_at| original_char_offset(line, folded_byte_at, &folded));
        }
        let ascii_folded = line.to_ascii_lowercase();
        let ascii_byte_at = ascii_folded
            .contains(&self.ascii)
            .then(|| ascii_folded.find(&self.ascii))
            .flatten();
        if self.needle_is_ascii {
            return ascii_byte_at.map(|byte_at| line[..byte_at].chars().count());
        }
        if let Some(byte_at) = ascii_byte_at {
            return Some(line[..byte_at].chars().count());
        }
        let unicode_folded = line.to_lowercase();
        if !unicode_folded.contains(&self.lowered) {
            return None;
        }
        // Anchor on the original line: a decided hit must always carry a
        // located offset; surfacing a zero here would be papering over a
        // defect, not handling one.
        unicode_folded
            .find(&self.lowered)
            .map(|folded_byte_at| original_char_offset(line, folded_byte_at, &unicode_folded))
    }
}

/// Emit the marked rows with their file prefix, bounded by the line cap.
fn emit_rows(
    emit: &[Option<char>],
    lines: &[&str],
    hit_at: &[usize],
    relative: &str,
    needle_chars: usize,
    line_cap: usize,
    out: &mut Vec<String>,
) {
    for (index, marker) in emit.iter().enumerate() {
        if out.len() >= line_cap {
            break;
        }
        let Some(m) = marker else { continue };
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

/// Map a byte offset in `folded` (the `to_lowercase()` image of `line`) back
/// onto a char offset in `line`.
///
/// Full Unicode folding can change byte lengths (ẞ U+1E9E → ß, İ U+0130 →
/// i + combining dot), so folded text is not a byte image of the original.
/// Walking both texts in lockstep by char keeps the mapping exact: consume one
/// original char per folded char, and one original char may cover several
/// folded ones (İ → i + U+0307). `folded_byte_at` is a byte offset in
/// `folded` (what `find` returns), not a char offset. A match starting inside
/// an expansion sequence anchors at the sequence's first original char — off
/// by at most one char, invisible in a 400-char cut window.
fn original_char_offset(line: &str, folded_byte_at: usize, folded: &str) -> usize {
    let mut folded_chars = folded[..folded_byte_at].chars().count();
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
#[path = "search_tests.rs"]
mod tests;
