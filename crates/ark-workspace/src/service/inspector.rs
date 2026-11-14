//! `WorkspaceInspector`: validate a candidate workspace path, then report status.
//!
//! Shape of the two public operations:
//!
//! * [`WorkspaceInspector::validate`] is the write-side gate: it rejects empty
//!   input, missing paths and non-directories, and returns the normalized
//!   absolute path a thread should be bound to. Its error messages are shown to
//!   the user as-is, so they name the offending path.
//! * [`WorkspaceInspector::status`] is the read-side probe used by
//!   `GET /v1/workspace/status`. It never fails: a missing directory, a non-git
//!   directory and an unusable git all degrade into contract fields.

use crate::domain::{
    base_status, clean_branch, clean_sha, count_status_lines, empty_status, expand_home,
    resolve_candidate,
};
use crate::service::git_runner::run_git;
use ark_common::{ArkError, Result};
use ark_contract::{WorkspaceDiff, WorkspaceDiffFile, WorkspaceFilePreview, WorkspaceStatus};
use std::path::{Path, PathBuf};

/// Largest file the preview endpoint will read at all. Mirrors the read tool's
/// legacy-decode bound: bigger files are build artifacts or media, and reading
/// them serves nobody.
pub const MAX_PREVIEW_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Text budget kept from a read: enough for any real document, small enough
/// that one file cannot eat the renderer or the HTTP response.
pub const PREVIEW_CONTENT_BUDGET: usize = 256 * 1024;

/// Canonicalized directory or a validation error naming it. On Windows the
/// `\\?\` verbatim prefix is stripped so prefix comparisons against a
/// user-visible root still work.
fn canonical_dir(path: &Path) -> Result<PathBuf> {
    let canonical = path.canonicalize().map_err(|_| {
        ArkError::validation(format!("cannot canonicalize path: {}", path.display()))
    })?;
    Ok(PathBuf::from(strip_verbatim_prefix(
        &canonical.to_string_lossy(),
    )))
}

/// Remove the Windows verbatim prefix (`\\?\C:\...`) so `starts_with` compares
/// user-visible spellings. No-op elsewhere.
fn strip_verbatim_prefix(text: &str) -> String {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    text.strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or_else(|| text.to_string())
}

/// Stateless-ish inspector. `base` is the anchor for relative candidates and is
/// captured once so a status call cannot drift when the process changes its
/// working directory (the Electron host does chdir during startup).
#[derive(Debug, Clone)]
pub struct WorkspaceInspector {
    base: Option<PathBuf>,
}

impl Default for WorkspaceInspector {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceInspector {
    /// Inspector anchored at the process working directory.
    pub fn new() -> Self {
        Self {
            base: std::env::current_dir().ok(),
        }
    }

    /// Inspector anchored at an explicit directory. Used by tests and by hosts
    /// that already know their project root.
    pub fn with_base(base: impl Into<PathBuf>) -> Self {
        Self {
            base: Some(base.into()),
        }
    }

    /// The resolution anchor, if one is known.
    pub fn base(&self) -> Option<&Path> {
        self.base.as_deref()
    }

    /// Validate and normalize a candidate workspace path.
    ///
    /// `~` expands to the user profile, a relative path resolves against
    /// [`Self::base`], and surrounding whitespace is ignored. Symlinks are
    /// followed, so a symlinked directory passes. The returned path is *not*
    /// canonicalized: on Windows `canonicalize` yields a `\\?\` verbatim prefix
    /// that would be shown verbatim in the UI.
    pub fn validate(&self, raw: &str) -> Result<PathBuf> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ArkError::validation("workspace path must not be empty"));
        }
        let resolved = resolve_candidate(self.base.as_deref(), trimmed);
        match std::fs::metadata(&resolved) {
            Ok(meta) if meta.is_dir() => Ok(resolved),
            Ok(_) => Err(ArkError::validation(format!(
                "workspace path is not a directory: {}",
                resolved.display()
            ))),
            Err(_) => Err(ArkError::validation(format!(
                "workspace path does not exist: {}",
                resolved.display()
            ))),
        }
    }

    /// Report the workspace status for `path`.
    ///
    /// An empty (or whitespace-only) path returns the contract's empty status
    /// without touching the filesystem, matching the TypeScript route which
    /// answers a `?path=`-less request with `exists: false` and `null` fields.
    pub fn status(&self, path: &str) -> WorkspaceStatus {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return empty_status();
        }
        let resolved = resolve_candidate(self.base.as_deref(), trimmed);
        if !resolved.is_dir() {
            // A file, a dangling symlink and a nonexistent path all mean "there
            // is no workspace here"; no git call is attempted.
            let mut status = base_status(path, false);
            status.exists = false;
            return status;
        }
        self.repository_status(path, &resolved)
    }

    /// Read a bounded, read-only preview of one file under `root`
    /// (requirements/workspace-file-preview.md).
    ///
    /// Containment is enforced in three layers, matching the `ark-tool` read
    /// tool's semantics without depending on that crate: lexical normalization,
    /// canonicalization, and a real-path check that resolves symlinks. A
    /// symlink pointing outside the root is therefore rejected even though the
    /// link itself lives inside. `~` is deliberately NOT expanded: the preview
    /// boundary is the workspace root, and the home directory is not inside it.
    pub fn read_file(&self, root: &Path, raw: &str) -> Result<WorkspaceFilePreview> {
        let path = raw.trim();
        if path.is_empty() {
            return Err(ArkError::validation("file path must not be empty"));
        }
        if expand_home(path) != *path {
            return Err(ArkError::validation(
                "file path must stay inside the workspace (`~` is not allowed)",
            ));
        }
        let canonical_root = canonical_dir(root)?;
        // Absolute candidates are accepted only if they already sit inside the
        // root; relative ones resolve against it directly.
        let candidate = {
            let expanded = PathBuf::from(path);
            if expanded.is_absolute() {
                expanded
            } else {
                canonical_root.join(expanded)
            }
        };
        // Fast lexical filter (no `..` may escape) before touching the disk.
        if candidate.components().any(|c| c.as_os_str() == "..") {
            return Err(ArkError::validation(format!(
                "file path escapes the workspace root: {path}"
            )));
        }
        if !candidate.exists() {
            return Err(ArkError::validation(format!("file does not exist: {path}")));
        }
        if candidate.is_dir() {
            return Err(ArkError::validation(format!(
                "file path is a directory: {path}"
            )));
        }
        // Real-path containment: symlinks must resolve inside the root too.
        let real = canonical_dir(&candidate)?;
        if !real.starts_with(&canonical_root) {
            return Err(ArkError::validation(format!(
                "file path escapes the workspace root: {path}"
            )));
        }
        let metadata = std::fs::metadata(&real)
            .map_err(|err| ArkError::validation(format!("cannot stat file {path}: {err}")))?;
        let size_bytes = metadata.len();
        if size_bytes > MAX_PREVIEW_FILE_BYTES {
            return Err(ArkError::validation(format!(
                "file is too large to preview ({size_bytes} bytes); limit is {MAX_PREVIEW_FILE_BYTES}"
            )));
        }
        let bytes = std::fs::read(&real)
            .map_err(|err| ArkError::validation(format!("cannot read file {path}: {err}")))?;
        let truncated = bytes.len() > PREVIEW_CONTENT_BUDGET;
        let content_bytes = if truncated {
            &bytes[..PREVIEW_CONTENT_BUDGET]
        } else {
            &bytes[..]
        };
        // Lossy decode keeps the preview usable for mixed encodings while the
        // replacement-char count tells the renderer to treat it as binary.
        let content = String::from_utf8_lossy(content_bytes);
        let binary = content.contains('\u{FFFD}');
        Ok(WorkspaceFilePreview {
            path: path.to_string(),
            content: content.into_owned(),
            truncated,
            binary,
            size_bytes,
        })
    }

    /// Read a bounded, side-effect-free Git diff for an already validated root.
    ///
    /// Untracked contents are intentionally never read: merely listing an
    /// untracked secret must not send it to the renderer. Git owns all parsing
    /// and command execution in this crate.
    pub fn diff(&self, root: &Path) -> Result<WorkspaceDiff> {
        if !root.is_dir() {
            return Err(ArkError::validation(format!(
                "workspace path is not a directory: {}",
                root.display()
            )));
        }
        let canonical = root.canonicalize().map_err(|_| {
            ArkError::validation(format!("cannot canonicalize workspace: {}", root.display()))
        })?;
        if !is_work_tree(&canonical) {
            return Err(ArkError::validation("workspace is not a git repository"));
        }
        let raw = git_stdout(&canonical, &["status", "--porcelain=v1", "-z"])
            .ok_or_else(|| ArkError::internal("git status failed"))?;
        let mut diff = WorkspaceDiff {
            head_sha: git_stdout(&canonical, &["rev-parse", "HEAD"])
                .and_then(|value| clean_sha(&value)),
            ..WorkspaceDiff::default()
        };
        for entry in parse_porcelain(&raw) {
            match entry.group {
                DiffGroup::Staged => diff.staged.push(entry.file),
                DiffGroup::Unstaged => diff.unstaged.push(entry.file),
                DiffGroup::Untracked => diff.untracked.push(entry.file),
            }
        }
        diff.file_count = (diff.staged.len() + diff.unstaged.len() + diff.untracked.len()) as u64;
        let mut staged = std::mem::take(&mut diff.staged);
        attach_patch(
            &canonical,
            &[
                "-c",
                "core.quotePath=false",
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
            ],
            &mut staged,
            &mut diff,
        );
        diff.staged = staged;
        let mut unstaged = std::mem::take(&mut diff.unstaged);
        attach_patch(
            &canonical,
            &[
                "-c",
                "core.quotePath=false",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
            ],
            &mut unstaged,
            &mut diff,
        );
        diff.unstaged = unstaged;
        Ok(diff)
    }

    fn repository_status(&self, raw: &str, dir: &Path) -> WorkspaceStatus {
        if !is_work_tree(dir) {
            return base_status(raw, false);
        }
        let mut status = base_status(raw, true);
        status.branch =
            git_stdout(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).and_then(|s| clean_branch(&s));
        status.head_sha = git_stdout(dir, &["rev-parse", "HEAD"]).and_then(|s| clean_sha(&s));
        if let Some(output) = git_stdout(dir, &["status", "--porcelain"]) {
            let count = count_status_lines(&output);
            status.is_dirty = Some(count > 0);
            status.file_change_count = Some(count);
        }
        status
    }
}

const MAX_DIFF_FILES: usize = 200;
const MAX_PATCH_BYTES: usize = 256 * 1024;

enum DiffGroup {
    Staged,
    Unstaged,
    Untracked,
}

struct ParsedDiffFile {
    group: DiffGroup,
    file: WorkspaceDiffFile,
}

fn parse_porcelain(raw: &str) -> Vec<ParsedDiffFile> {
    let mut parts = raw.split('\0').filter(|part| !part.is_empty());
    let mut result = Vec::new();
    while let Some(record) = parts.next() {
        if result.len() >= MAX_DIFF_FILES {
            break;
        }
        if record.len() < 4 {
            continue;
        }
        let code = &record[..2];
        let path = record[3..].to_string();
        if code == "??" {
            result.push(ParsedDiffFile {
                group: DiffGroup::Untracked,
                file: diff_file(path, code, None),
            });
            continue;
        }
        let renamed =
            matches!(code.as_bytes()[0], b'R' | b'C') || matches!(code.as_bytes()[1], b'R' | b'C');
        let previous_path = renamed.then(|| parts.next().unwrap_or_default().to_string());
        if code.as_bytes()[0] != b' ' {
            result.push(ParsedDiffFile {
                group: DiffGroup::Staged,
                file: diff_file(path.clone(), code, previous_path.clone()),
            });
        }
        if code.as_bytes()[1] != b' ' {
            result.push(ParsedDiffFile {
                group: DiffGroup::Unstaged,
                file: diff_file(path, code, previous_path),
            });
        }
    }
    result
}

fn diff_file(path: String, status: &str, previous_path: Option<String>) -> WorkspaceDiffFile {
    WorkspaceDiffFile {
        path,
        status: status.to_string(),
        previous_path,
        binary: false,
        patch: None,
        truncated: false,
    }
}

fn attach_patch(
    root: &Path,
    args: &[&str],
    files: &mut [WorkspaceDiffFile],
    diff: &mut WorkspaceDiff,
) {
    if files.is_empty() || diff.truncated {
        return;
    }
    let Some(output) = git_stdout(root, args) else {
        return;
    };
    let available = MAX_PATCH_BYTES.saturating_sub(diff.patch_bytes as usize);
    if available == 0 {
        diff.truncated = true;
        return;
    }
    let bytes = output.as_bytes();
    let (patch, truncated) = if bytes.len() > available {
        (
            String::from_utf8_lossy(&bytes[..available]).into_owned(),
            true,
        )
    } else {
        (output, false)
    };
    diff.truncated |= truncated;
    // Each file row must carry only its own `diff --git` segment: a renderer
    // expands a row to show that file's changes, so handing every row the whole
    // group diff would repeat the other files' edits under each path.
    let segments = split_patch_segments(&patch);
    let mut assigned_bytes = 0usize;
    for file in files.iter_mut() {
        // The byte cap truncates the group output, not a single segment, so a
        // missing/short segment for one row still means "output was cut".
        file.truncated = truncated;
        let Some(segment) = segments.iter().find(|segment| segment.path == file.path) else {
            continue;
        };
        let binary =
            segment.text.contains("Binary files ") || segment.text.contains("GIT binary patch");
        file.binary = binary;
        file.patch = (!binary).then(|| segment.text.clone());
        // Only assigned segments are kept, so the byte counter reports the
        // payload actually shipped to the renderer, not the raw git output.
        assigned_bytes += segment.text.len();
    }
    diff.patch_bytes += assigned_bytes as u64;
}

/// A per-file slice of a group `git diff`, keyed by the path its headers name.
struct PatchSegment {
    path: String,
    text: String,
}

/// Split group `git diff` output into per-file segments.
///
/// Boundaries are lines *starting* with `diff --git ` (a content line can
/// contain that substring mid-hunk). The key path prefers `+++ b/<path>`
/// (every content change), then `--- a/<path>` (deletions, `+++` is
/// `/dev/null`), then the b-side of the `diff --git a/X b/Y` header itself —
/// that last fallback is what attributes binary-only segments, which carry no
/// `---`/`+++` pair at all. The caller runs git with `core.quotePath=false`,
/// so non-ASCII paths arrive raw; `unquote_path` only handles the c-style
/// quoting git still applies to control characters and embedded quotes.
fn split_patch_segments(patch: &str) -> Vec<PatchSegment> {
    let lines: Vec<&str> = patch.split_inclusive('\n').collect();
    let mut bounds: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("diff --git "))
        .map(|(idx, _)| idx)
        .collect();
    bounds.push(lines.len());

    let mut segments = Vec::new();
    for window in bounds.windows(2) {
        let (start, end) = (window[0], window[1]);
        if start == end {
            continue;
        }
        let text = lines[start..end].concat();
        if let Some(path) = segment_path(&lines[start..end]) {
            segments.push(PatchSegment { path, text });
        }
    }
    segments
}

/// Attribute one segment to a file path, preferring the most specific header.
fn segment_path(segment: &[&str]) -> Option<String> {
    let mut fallback: Option<String> = None;
    for line in segment {
        let line = line.trim_end();
        if let Some(path) = header_path(line, "+++ b/") {
            return Some(path);
        }
        if fallback.is_none() {
            if let Some(path) = header_path(line, "rename to ") {
                // A pure rename (100% similarity) has no `---`/`+++` pair, and
                // the `diff --git` header is ambiguous when a path contains
                // ` b/`; `rename to` names the new path unambiguously.
                fallback = Some(path);
            } else if let Some(path) = header_path(line, "--- a/") {
                fallback = Some(path);
            } else if let Some(path) = diff_git_header_path(line) {
                fallback = Some(path);
            }
        }
    }
    fallback
}

/// Extract the path from a `--- a/x` / `+++ b/x` / `rename to x` header.
///
/// Handles both the raw form and the c-quoted form git still uses when the
/// path itself contains quotes (`+++ "b/name\""`).
fn header_path(line: &str, marker: &str) -> Option<String> {
    let body = line.strip_prefix(marker)?;
    if let Some(raw) = body.strip_prefix("b/").or_else(|| body.strip_prefix("a/")) {
        return Some(unquote_path(raw));
    }
    let quoted = body.strip_prefix('"')?;
    let end = quoted.rfind('"')?;
    let inner = quoted[..end]
        .strip_prefix("b/")
        .or_else(|| quoted[..end].strip_prefix("a/"))?;
    Some(unescape_c_style(inner))
}

/// Path from a `diff --git a/<old> b/<new>` header, in raw or quoted form.
///
/// For identical paths pick the reading where both sides match (spaces in a
/// path make the line ambiguous); renames fall back to the b-side. This is
/// the only header binary-only segments carry, so it is what attributes them.
fn diff_git_header_path(line: &str) -> Option<String> {
    let rest = line.strip_prefix("diff --git ")?;
    if let Some(raw) = rest.strip_prefix("a/") {
        let mut search_from = 0;
        while let Some(offset) = raw[search_from..].find(" b/") {
            let at = search_from + offset;
            let (old, new) = (&raw[..at], &raw[at + 3..]);
            if old == new {
                return Some(unquote_path(old));
            }
            search_from = at + 1;
        }
        let at = raw.rfind(" b/")?;
        return Some(unquote_path(&raw[at + 3..]));
    }
    let quoted = rest.strip_prefix('"')?;
    let close = unescaped_quote(quoted)?;
    let old = &quoted[..close];
    let new_side = quoted[close + 1..].trim_start();
    let new_side = new_side.strip_prefix('"').unwrap_or(new_side);
    let new_side = new_side.strip_prefix("b/").unwrap_or(new_side);
    let new_side = new_side.strip_suffix('"').unwrap_or(new_side);
    if old == new_side {
        return Some(unescape_c_style(old));
    }
    Some(unescape_c_style(new_side))
}

/// First `"` that actually closes a quoted git path: escaped characters
/// (`\"`, `\\`) are skipped, so an embedded `\"` cannot end the scan early.
fn unescaped_quote(quoted: &str) -> Option<usize> {
    let bytes = quoted.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Some(index),
            _ => index += 1,
        }
    }
    None
}

/// Git still c-quotes control characters and embedded quotes in diff headers
/// even with `core.quotePath=false`; strip the wrapper and decode the escapes.
fn unquote_path(path: &str) -> String {
    let trimmed = path.trim_end();
    match trimmed
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
    {
        Some(inner) => unescape_c_style(inner),
        None => trimmed.to_string(),
    }
}

/// Decode the c-style escapes git uses inside quoted paths (`\"`, `\\`,
/// `\n`/`\t`/`\r` and `\NNN` octal bytes).
fn unescape_c_style(quoted: &str) -> String {
    let mut out = String::with_capacity(quoted.len());
    let mut chars = quoted.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(first) if ('0'..='7').contains(&first) => {
                let mut value = first.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.peek().and_then(|next| next.to_digit(8)) {
                        Some(digit) if value * 8 + digit <= u8::MAX as u32 => {
                            value = value * 8 + digit;
                            chars.next();
                        }
                        _ => break,
                    }
                }
                // With `core.quotePath=false` octal escapes only appear for
                // control characters and quotes, never for UTF-8 sequences.
                out.push(char::from_u32(value).unwrap_or('\u{FFFD}'));
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// `git rev-parse --is-inside-work-tree` prints exactly `true` inside a work
/// tree. Inside `.git`, or outside any repository, git fails and we report
/// "not a repository" rather than guessing.
fn is_work_tree(dir: &Path) -> bool {
    git_stdout(dir, &["rev-parse", "--is-inside-work-tree"])
        .map(|out| out.trim() == "true")
        .unwrap_or(false)
}

fn git_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    run_git(dir, args).map(|output| output.stdout)
}

#[cfg(test)]
#[path = "inspector_tests.rs"]
mod tests;
