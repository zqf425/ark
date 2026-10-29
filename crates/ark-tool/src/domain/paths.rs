//! Workspace path containment (design §7.3).
//!
//! Every filesystem tool resolves its argument through [`resolve`], which applies
//! **two** checks and refuses the path if either one fails:
//!
//! 1. a lexical check against the canonical normalizer in `ark_common::path`,
//!    which rejects `..` traversal and sibling directories with a shared prefix;
//! 2. a real-path check that canonicalizes the deepest existing ancestor and
//!    requires it to stay inside the canonicalized root.
//!
//! The lexical check alone is not enough: a symlink or Windows junction placed
//! inside the workspace passes the string test while the filesystem call follows
//! it to an arbitrary target. The real-path check is what closes that.

use ark_common::{path, ArkError, Result};
use std::path::{Path, PathBuf};

/// Resolve `input` against `root`, rejecting escapes.
///
/// `input` may be absolute or relative; a relative path is joined to `root`
/// before normalization. Both the lexical check and the real-path check run
/// before the result is handed to a filesystem call.
pub fn resolve(root: &Path, input: &str) -> Result<PathBuf> {
    let root_key = path::normalize(&root.to_string_lossy());
    if root_key.is_empty() {
        return Err(ArkError::validation("workspace root is empty"));
    }
    let candidate = if Path::new(input).is_absolute() {
        input.to_string()
    } else {
        format!("{root_key}/{input}")
    };
    let normalized = path::normalize(&candidate);
    if !is_inside(&root_key, &normalized) {
        return Err(
            ArkError::validation(format!("path `{input}` escapes the workspace root"))
                .with_detail(format!("resolved to `{normalized}`")),
        );
    }
    let resolved = PathBuf::from(normalized.replace('/', std::path::MAIN_SEPARATOR_STR));
    // A path can be lexically inside the root yet resolve outside it once links
    // are followed, so the filesystem gets the final say.
    ensure_real_path_inside(root, &resolved, input)?;
    Ok(resolved)
}

/// Reject a path whose existing portion resolves outside `root` via a link.
///
/// `canonicalize` follows symlinks and Windows junctions, so comparing the
/// canonicalized deepest existing ancestor of `candidate` against the
/// canonicalized root catches both. A not-yet-existing target is checked through
/// its nearest existing parent, which also covers `write` creating directories.
fn ensure_real_path_inside(root: &Path, candidate: &Path, input: &str) -> Result<()> {
    // The root itself may not exist yet (tests use synthetic roots, and a fresh
    // data dir is created later); with no real root there is nothing to compare.
    let Ok(real_root) = std::fs::canonicalize(root) else {
        return Ok(());
    };
    let mut probe = candidate;
    let real_candidate = loop {
        match std::fs::canonicalize(probe) {
            Ok(real) => break Some(real),
            Err(_) => match probe.parent() {
                Some(parent) if parent != probe => probe = parent,
                _ => break None,
            },
        }
    };
    let Some(real_candidate) = real_candidate else {
        return Ok(());
    };
    if is_real_inside(&real_root, &real_candidate) {
        return Ok(());
    }
    Err(
        ArkError::validation(format!("path `{input}` escapes the workspace root")).with_detail(
            format!(
                "`{}` resolves to `{}`, outside `{}`",
                candidate.display(),
                real_candidate.display(),
                real_root.display()
            ),
        ),
    )
}

/// Component-aware containment over canonical paths.
///
/// Windows paths are case-insensitive, so the comparison is done on a
/// separator-normalized, lowercased form. Both sides come from `canonicalize`
/// and therefore carry the same `\\?\` prefix.
fn is_real_inside(real_root: &Path, real_candidate: &Path) -> bool {
    let root = comparable(real_root);
    let candidate = comparable(real_candidate);
    if candidate == root {
        return true;
    }
    candidate.starts_with(&format!("{}/", root.trim_end_matches('/')))
}

fn comparable(path: &Path) -> String {
    path::strip_verbatim(&path.to_string_lossy())
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

/// Whether `normalized` is `root` itself or a descendant of it.
fn is_inside(root: &str, normalized: &str) -> bool {
    if normalized == root {
        return true;
    }
    let prefix = format!("{}/", root.trim_end_matches('/'));
    normalized.starts_with(&prefix)
}

/// Reject a path that is not a plain single path segment, e.g. a glob that
/// would widen a search outside `root`.
pub fn require_simple_segment(name: &str, what: &str) -> Result<()> {
    if name.contains("..") || name.starts_with('/') || name.contains(':') {
        return Err(ArkError::validation(format!(
            "{what} must be a plain segment, got `{name}`"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("D:/ws/project")
    }

    #[test]
    fn relative_paths_resolve_inside_the_root() {
        let resolved = resolve(&root(), "src/main.rs").unwrap();
        assert!(resolved.to_string_lossy().ends_with("main.rs"));
    }

    #[test]
    fn dot_segments_are_collapsed_before_the_check() {
        let resolved = resolve(&root(), "src/../Cargo.toml").unwrap();
        let text = resolved.to_string_lossy().replace('\\', "/");
        assert!(text.ends_with("project/Cargo.toml"), "got {text}");
    }

    #[test]
    fn parent_escape_is_rejected() {
        assert!(resolve(&root(), "../../etc/passwd").is_err());
        assert!(resolve(&root(), "src/../../secret").is_err());
    }

    #[test]
    fn absolute_path_outside_the_root_is_rejected() {
        assert!(resolve(&root(), "C:/Windows/System32/drivers/etc/hosts").is_err());
    }

    #[test]
    fn absolute_path_inside_the_root_is_accepted() {
        assert!(resolve(&root(), "D:/ws/project/a.txt").is_ok());
    }

    #[test]
    fn verbatim_root_accepts_relative_dot_and_plain_absolute_paths() {
        let root = PathBuf::from(r"\\?\D:\ws\project");
        assert!(resolve(&root, "src/main.rs").is_ok());
        assert!(resolve(&root, ".").is_ok());
        assert!(resolve(&root, r"D:\ws\project\Cargo.toml").is_ok());
    }

    #[test]
    fn sibling_with_a_shared_prefix_is_rejected() {
        assert!(resolve(&root(), "D:/ws/project-evil/a.txt").is_err());
    }

    #[test]
    fn simple_segments_reject_traversal_and_drive_letters() {
        assert!(require_simple_segment("src", "directory").is_ok());
        assert!(require_simple_segment("../x", "directory").is_err());
        assert!(require_simple_segment("C:", "directory").is_err());
    }

    /// One throwaway directory outside the workspace, used as the link target.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ark-paths-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir is creatable");
        dir
    }

    #[test]
    fn an_existing_path_inside_the_root_still_resolves() {
        // The real-path check must not reject ordinary files just because the
        // root is a real directory rather than a synthetic string.
        let dir = temp_dir("inside");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        let resolved = resolve(&dir, "src/main.rs").expect("an in-root file must resolve");
        assert!(resolved.ends_with("main.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_not_yet_existing_file_under_an_existing_parent_resolves() {
        // `write` legitimately targets files that do not exist yet; the check
        // walks up to the nearest existing ancestor instead of failing.
        let dir = temp_dir("newfile");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        let resolved = resolve(&dir, "nested/brand-new.txt").expect("a new file must resolve");
        assert!(resolved.ends_with("brand-new.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Create a directory link inside `root` pointing at `target`.
    ///
    /// On Windows a symlink needs Developer Mode or elevation, while a junction
    /// needs neither; on Unix a symlink is always available. Returns `false`
    /// when the platform refuses, so the test can skip rather than fail on an
    /// environment it cannot control.
    #[cfg(windows)]
    fn link_dir(target: &Path, link: &Path) -> bool {
        use std::process::Command;
        Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|out| out.status.success() && link.exists())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    fn link_dir(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[test]
    fn a_link_inside_the_root_cannot_reach_outside_it() {
        // The string `link/secret.txt` is lexically inside the root, but the
        // filesystem would follow the link out of it. This is the case the
        // lexical check alone cannot see (P0-3).
        let root = temp_dir("link-root");
        let outside = temp_dir("link-outside");
        std::fs::write(outside.join("secret.txt"), "classified").unwrap();

        let link = root.join("escape");
        if !link_dir(&outside, &link) {
            // No link privilege on this host: the lexical tests still cover the
            // traversal half, so skipping is honest rather than failing.
            let _ = std::fs::remove_dir_all(&root);
            let _ = std::fs::remove_dir_all(&outside);
            return;
        }

        let err = resolve(&root, "escape/secret.txt")
            .expect_err("a link leading outside the root must be refused");
        assert!(
            err.message.contains("escapes the workspace root"),
            "{}",
            err.message
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
