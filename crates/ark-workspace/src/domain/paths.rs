//! Home expansion and relative-path resolution (design 7.9).
//!
//! The renderer sends whatever the user typed into the workspace field, so this
//! module owns the two normalizations every later git call depends on:
//!
//! * `~` / `~/sub` expands to the host home directory;
//! * a relative candidate resolves against the inspector base (the process
//!   working directory by default), never against the caller's imagination.
//!
//! No filesystem access happens here, which keeps the rules testable without a
//! fixture directory.

use std::path::{Path, PathBuf};

/// Resolve the current user's home directory.
///
/// Windows sets `USERPROFILE`; POSIX sets `HOME`. Both are consulted in that
/// order so a Git-Bash session on Windows still resolves to the Windows profile
/// rather than to a mount point that git cannot enter.
pub fn home_dir() -> Option<PathBuf> {
    for key in ["USERPROFILE", "HOME"] {
        if let Some(value) = std::env::var_os(key) {
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

/// Expand a leading `~` and trim surrounding whitespace.
///
/// A bare `~` and the two separator spellings `~/x` / `~\x` are handled. When no
/// home directory is known the input is returned unchanged rather than dropped:
/// the caller then fails validation with the path it was actually given, which is
/// a more useful error than a silently empty string.
pub fn expand_home(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if trimmed == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(trimmed));
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(trimmed)
}

/// Expand `~` and resolve a relative result against `base`.
///
/// `base` is `None` when the host could not determine its working directory; the
/// relative path is then returned as-is so the caller's `is_dir()` check fails
/// loudly instead of resolving against an unrelated root.
pub fn resolve_candidate(base: Option<&Path>, raw: &str) -> PathBuf {
    let expanded = expand_home(raw);
    if expanded.is_absolute() {
        return expanded;
    }
    let anchor = match base {
        Some(dir) => Some(dir.to_path_buf()),
        None => std::env::current_dir().ok(),
    };
    match anchor {
        Some(dir) => dir.join(expanded),
        None => expanded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_home_replaces_bare_tilde() {
        let home = match home_dir() {
            Some(home) => home,
            None => return,
        };
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("  ~  "), home);
    }

    #[test]
    fn expand_home_replaces_tilde_prefix() {
        let home = match home_dir() {
            Some(home) => home,
            None => return,
        };
        assert_eq!(expand_home("~/projects"), home.join("projects"));
        assert_eq!(expand_home(r"~\projects"), home.join("projects"));
    }

    #[test]
    fn expand_home_leaves_regular_paths_alone() {
        assert_eq!(
            expand_home("  relative/dir "),
            PathBuf::from("relative/dir")
        );
        // A `~` that is not a leading segment belongs to the name.
        assert_eq!(expand_home("a/~/b"), PathBuf::from("a/~/b"));
    }

    #[test]
    fn resolve_candidate_joins_relative_against_base() {
        let base = PathBuf::from("/tmp/base");
        assert_eq!(
            resolve_candidate(Some(&base), "child"),
            PathBuf::from("/tmp/base").join("child")
        );
    }

    #[test]
    fn resolve_candidate_keeps_absolute_paths() {
        let base = PathBuf::from("/tmp/base");
        #[cfg(windows)]
        let absolute = PathBuf::from(r"C:\work\repo");
        #[cfg(not(windows))]
        let absolute = PathBuf::from("/work/repo");
        assert_eq!(
            resolve_candidate(Some(&base), &absolute.to_string_lossy()),
            absolute
        );
    }
}
