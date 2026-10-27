//! Skill domain model: one discovered package, plus the id/summary helpers it
//! shares with the service layer.

use std::path::{Path, PathBuf};

use ark_contract::{SkillSource, SkillSummary, SkillTriggers};

/// A skill package that passed discovery, with its entry text already read.
///
/// `entry_text` is `Option` on purpose: a package whose `SKILL.md` disappeared
/// between discovery and read is still listed (the user can see it and fix it)
/// but cannot contribute instructions.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSkill {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub root: PathBuf,
    pub entry_path: PathBuf,
    pub entry_text: Option<String>,
    pub triggers: SkillTriggers,
    pub allowed_tools: Vec<String>,
    pub priority: i32,
    pub source: SkillSource,
    /// True when the package had no `skill.json` and was read as a bare
    /// `SKILL.md`.
    pub legacy: bool,
}

impl LoadedSkill {
    /// Wire-facing summary for `GET /v1/skills` and the thread skill view.
    pub fn summary(&self) -> SkillSummary {
        SkillSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            version: self.version.clone(),
            root: path_display(&self.root),
            entry_path: path_display(&self.entry_path),
            priority: self.priority,
            source: self.source,
            legacy: self.legacy,
        }
    }
}

/// Render a path for the wire, using forward slashes so a Windows path stays
/// readable and comparable with the TypeScript runtime's `join` output.
pub fn path_display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Stable skill id derived from a manifest id or a directory name.
///
/// Lowercases, keeps letters/digits/`_`/`-`, collapses everything else to a
/// single `-`, and falls back to `skill` so an id is never empty.
pub fn slug(value: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in value.trim().chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
        } else {
            pending_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "skill".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_normalizes_separators_and_case() {
        assert_eq!(slug("PDF Tools"), "pdf-tools");
        assert_eq!(slug("  --My_Skill--  "), "my_skill");
        assert_eq!(slug("a...b"), "a-b");
        assert_eq!(slug("***"), "skill");
    }

    #[test]
    fn slug_keeps_cjk_letters() {
        assert_eq!(slug("报表工具"), "报表工具");
    }

    #[test]
    fn path_display_uses_forward_slashes() {
        assert_eq!(path_display(Path::new(r"C:\ws\skills\a")), "C:/ws/skills/a");
    }
}
