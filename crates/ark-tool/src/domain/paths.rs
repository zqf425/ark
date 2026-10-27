//! Workspace path containment (design §7.3).
//!
//! Every filesystem tool resolves its argument through [`resolve`], which uses
//! the single canonical normalizer in `ark_common::path` and then refuses any
//! result that is not inside the workspace root. There is exactly one place that
//! decides "is this path allowed", so one mistake cannot be repeated per tool.

use ark_common::{path, ArkError, Result};
use std::path::{Path, PathBuf};

/// Resolve `input` against `root`, rejecting escapes.
///
/// `input` may be absolute or relative; a relative path is joined to `root`
/// before normalization. Symlinks are not followed here on purpose: the check is
/// lexical, and the process runs with the workspace as its only writable area.
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
    Ok(PathBuf::from(
        normalized.replace('/', std::path::MAIN_SEPARATOR_STR),
    ))
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
    fn sibling_with_a_shared_prefix_is_rejected() {
        assert!(resolve(&root(), "D:/ws/project-evil/a.txt").is_err());
    }

    #[test]
    fn simple_segments_reject_traversal_and_drive_letters() {
        assert!(require_simple_segment("src", "directory").is_ok());
        assert!(require_simple_segment("../x", "directory").is_err());
        assert!(require_simple_segment("C:", "directory").is_err());
    }
}
