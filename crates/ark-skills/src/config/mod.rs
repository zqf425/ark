//! Skill runtime configuration.
//!
//! Shape mirrors `SkillsCapabilityConfig` in the TypeScript runtime, plus the
//! workspace-relative directories (`workspace_dirs`) that the Rust runtime scans
//! in addition to the absolute base roots.

use std::path::{Path, PathBuf};

/// Default number of skills activated for one turn.
pub const DEFAULT_ACTIVE_LIMIT: usize = 3;
/// Default per-turn budget for injected entry text.
pub const DEFAULT_INSTRUCTION_BUDGET_BYTES: usize = 24_000;
/// Default budget for the rendered skill catalog.
pub const DEFAULT_CATALOG_BUDGET_BYTES: usize = 8_000;

/// Skill discovery and activation configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillsConfig {
    /// Master switch. When false every view is empty and no turn gets skills.
    pub enabled: bool,
    /// Base roots (usually global). `~` is expanded at scan time.
    pub roots: Vec<PathBuf>,
    /// Workspace-relative directories appended to the base roots.
    pub workspace_dirs: Vec<String>,
    /// Maximum number of skills activated for one turn.
    pub active_limit: usize,
    /// Byte budget for injected entry text.
    pub instruction_budget_bytes: usize,
    /// Byte budget for the rendered catalog text.
    pub catalog_budget_bytes: usize,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: vec![home_dir().join(".ark").join("skills")],
            workspace_dirs: vec![".agents/skills".to_string(), "skills".to_string()],
            active_limit: DEFAULT_ACTIVE_LIMIT,
            instruction_budget_bytes: DEFAULT_INSTRUCTION_BUDGET_BYTES,
            catalog_budget_bytes: DEFAULT_CATALOG_BUDGET_BYTES,
        }
    }
}

impl SkillsConfig {
    /// Absolute base roots for `workspace`: configured roots plus the
    /// workspace-relative directories, de-duplicated in configured order.
    ///
    /// A relative configured root is resolved against `workspace` so a config
    /// written by hand still points somewhere meaningful.
    pub fn base_roots(&self, workspace: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for root in &self.roots {
            let expanded = expand_home(root);
            let absolute = if expanded.is_absolute() {
                expanded
            } else {
                workspace.join(expanded)
            };
            push_unique(&mut out, normalize_lexically(&absolute));
        }
        for dir in &self.workspace_dirs {
            let trimmed = dir.trim();
            if trimmed.is_empty() {
                continue;
            }
            push_unique(&mut out, normalize_lexically(&workspace.join(trimmed)));
        }
        out
    }
}

fn push_unique(out: &mut Vec<PathBuf>, path: PathBuf) {
    if !out.iter().any(|existing| existing == &path) {
        out.push(path);
    }
}

/// Expand a leading `~` using the current user's home directory.
pub fn expand_home(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let Some(rest) = text.strip_prefix('~') else {
        return path.to_path_buf();
    };
    if rest.is_empty() {
        return home_dir();
    }
    let rest = rest.trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        home_dir()
    } else {
        home_dir().join(rest)
    }
}

/// Current user's home directory.
///
/// `std` has no home lookup, so this reads the environment variables Windows and
/// POSIX actually set and falls back to the current directory instead of
/// failing: a missing home must never break skill discovery.
pub fn home_dir() -> PathBuf {
    for key in ["USERPROFILE", "HOME", "HOMEPATH"] {
        if let Some(value) = std::env::var_os(key) {
            let path = PathBuf::from(value);
            if !path.as_os_str().is_empty() {
                return path;
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Drop `.` segments and fold `..` when it stays inside the path.
fn normalize_lexically(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_contract() {
        let config = SkillsConfig::default();
        assert!(config.enabled);
        assert_eq!(config.active_limit, 3);
        assert_eq!(config.instruction_budget_bytes, 24_000);
        assert_eq!(config.catalog_budget_bytes, 8_000);
        assert_eq!(config.workspace_dirs, vec![".agents/skills", "skills"]);
        assert_eq!(config.roots.len(), 1);
    }

    #[test]
    fn base_roots_include_workspace_dirs() {
        let config = SkillsConfig {
            roots: vec![PathBuf::from("/global/skills")],
            ..SkillsConfig::default()
        };
        let roots = config.base_roots(Path::new("/ws"));
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/global/skills"),
                PathBuf::from("/ws/.agents/skills"),
                PathBuf::from("/ws/skills"),
            ]
        );
    }

    #[test]
    fn base_roots_are_deduplicated_and_ignore_blank_dirs() {
        let config = SkillsConfig {
            roots: vec![PathBuf::from("/ws/skills")],
            workspace_dirs: vec!["skills".to_string(), "  ".to_string()],
            ..SkillsConfig::default()
        };
        let roots = config.base_roots(Path::new("/ws"));
        assert_eq!(roots, vec![PathBuf::from("/ws/skills")]);
    }

    #[test]
    fn lexical_normalization_drops_dot_segments() {
        let normalized = normalize_lexically(Path::new("/ws/./a/../skills"));
        assert_eq!(normalized, PathBuf::from("/ws/skills"));
    }
}
