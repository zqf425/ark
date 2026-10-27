//! File-root containment for plugin private storage (design 7.5.5).
//!
//! A plugin's only filesystem view is `data/plugins/<plugin_id>/`, addressed by
//! relative paths. Two escapes are rejected here:
//!
//! * **traversal** - `..`, absolute paths, drive letters, UNC prefixes;
//! * **link escape** - a symlink (or junction) inside the root that points
//!   outside it. Checked on the canonicalized parent, so the check also covers
//!   a link created after the plugin was loaded.

use ark_common::{ArkError, Result};
use std::path::{Component, Path, PathBuf};

/// Lexical validation of a relative path. No filesystem access.
pub fn sanitize_relative(rel: &str) -> Result<PathBuf> {
    if rel.trim().is_empty() {
        return Err(deny("path is empty", "plugin_path_empty"));
    }
    if rel.len() > 1024 {
        return Err(deny("path is too long", "plugin_path_invalid"));
    }
    if rel.contains('\0') {
        return Err(deny("path contains a NUL byte", "plugin_path_invalid"));
    }
    if rel.contains(':') {
        return Err(deny(
            "path must not carry a drive or stream prefix",
            "plugin_path_traversal",
        ));
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        return Err(deny("path must be relative", "plugin_path_traversal"));
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(deny("path must not contain ..", "plugin_path_traversal"))
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(deny("path must be relative", "plugin_path_traversal"))
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(deny(
            "path resolves to the root itself",
            "plugin_path_empty",
        ));
    }
    Ok(out)
}

/// Resolve a relative path against `root`, then prove the result stays inside.
///
/// The deepest existing ancestor is canonicalized so a not-yet-created file is
/// still checked against a symlinked parent directory.
pub fn resolve_within_root(root: &Path, rel: &str) -> Result<PathBuf> {
    let relative = sanitize_relative(rel)?;
    let root_canonical = root.canonicalize().map_err(|err| {
        ArkError::store(format!(
            "plugin root {} is not usable: {err}",
            root.display()
        ))
        .with_detail("plugin_root_unavailable")
    })?;
    let candidate = root_canonical.join(&relative);

    let mut probe = candidate.clone();
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let anchor = loop {
        match probe.canonicalize() {
            Ok(canonical) => break canonical,
            Err(_) => {
                let name = probe.file_name().map(|n| n.to_os_string());
                match name {
                    Some(name) => {
                        suffix.push(name);
                        if !probe.pop() {
                            return Err(deny(
                                "path has no reachable ancestor",
                                "plugin_path_traversal",
                            ));
                        }
                    }
                    None => {
                        return Err(deny(
                            "path has no reachable ancestor",
                            "plugin_path_traversal",
                        ))
                    }
                }
            }
        }
    };

    if !anchor.starts_with(&root_canonical) {
        return Err(deny("path escapes the plugin root", "plugin_path_escape"));
    }

    // Rebuild the not-yet-existing tail on top of the canonical ancestor, then
    // re-check: a symlink segment could still have been resolved above.
    let mut resolved = anchor;
    for part in suffix.iter().rev() {
        if part == ".." {
            return Err(deny("path must not contain ..", "plugin_path_traversal"));
        }
        resolved.push(part);
    }
    if !resolved.starts_with(&root_canonical) {
        return Err(deny("path escapes the plugin root", "plugin_path_escape"));
    }
    Ok(resolved)
}

fn deny(message: &str, code: &str) -> ArkError {
    ArkError::validation(message.to_string()).with_detail(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ark-plugin-paths-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn simple_relative_paths_are_accepted() {
        let root = temp_root("simple");
        let resolved = resolve_within_root(&root, "notes/today.txt").unwrap();
        assert!(resolved.starts_with(root.canonicalize().unwrap()));
        assert!(resolved.ends_with("notes\\today.txt") || resolved.ends_with("notes/today.txt"));
    }

    #[test]
    fn traversal_is_rejected_lexically() {
        let root = temp_root("traversal");
        for bad in [
            "../escape.txt",
            "a/../../escape.txt",
            "/etc/passwd",
            "C:\\Windows\\x",
        ] {
            assert!(sanitize_relative(bad).is_err(), "{bad} must be rejected");
            assert!(
                resolve_within_root(&root, bad).is_err(),
                "{bad} must be rejected"
            );
        }
        for bad in ["..", "sub/..", "", "  "] {
            assert!(
                resolve_within_root(&root, bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn a_deep_traversal_cannot_reach_the_parent_directory() {
        let root = temp_root("deep");
        fs::create_dir_all(root.join("a/b")).unwrap();
        let err = resolve_within_root(&root, "a/b/../../../outside.txt").unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("plugin_path_traversal"));
    }

    #[test]
    fn a_symlinked_directory_inside_the_root_cannot_escape() {
        let root = temp_root("symlink");
        let outside = temp_root("symlink-outside");
        fs::write(outside.join("secret.txt"), b"secret").unwrap();
        let link = root.join("link");

        #[cfg(unix)]
        let created = std::os::unix::fs::symlink(&outside, &link).is_ok();
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_dir(&outside, &link).is_ok();

        if !created {
            // Creating a symlink needs privilege on Windows. The containment
            // rule is still exercised by the lexical tests above; skip loudly
            // rather than assert something that cannot be set up here.
            eprintln!("skipping symlink escape test: cannot create a directory symlink here");
            return;
        }
        let err = resolve_within_root(&root, "link/secret.txt").unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("plugin_path_escape"));
    }

    #[test]
    fn an_absolute_path_never_escapes_the_root() {
        let root = temp_root("absolute");
        let target = root.join("inside.txt");
        let err = resolve_within_root(&root, &target.to_string_lossy()).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("plugin_path_traversal"));
    }
}
