//! Catalog rendering: the short "skills exist" listing handed to the model.
//!
//! The catalog is byte-budgeted and rendered on demand, never stored: the same
//! skill set with the same budget always produces the same bytes, which keeps
//! the prompt prefix stable for cache hits.

use ark_common::text::truncate_utf8;

use crate::domain::skill::LoadedSkill;

const HEADER: &str = "## Skills\n\
A skill is a reusable set of instructions stored on disk. The skills below are \
available. When a user request matches one, call the `load_skill` tool with its \
id before acting, then follow the returned instructions.";

const SECTION: &str = "### Available skills";

const FOOTER: &str = "### How to use skills\n\
- A skill activates automatically when the user mentions its id or trips one of \
its triggers; its instructions are then injected for that turn.\n\
- Otherwise, if a request clearly matches a skill above, call the `load_skill` \
tool with its id to pull the full instructions, then follow them.";

/// Render the catalog for `skills`, capped at `budget_bytes`.
///
/// Returns `None` when there is nothing to list or the budget cannot fit even a
/// single line, so a no-skills turn stays byte-identical to the disabled case.
pub fn render_catalog(skills: &[LoadedSkill], budget_bytes: usize) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let fixed = format!("{HEADER}\n\n{SECTION}\n\n{FOOTER}");
    let mut used = fixed.len();
    let mut lines: Vec<String> = Vec::new();
    let mut dropped = 0usize;
    for skill in skills {
        let line = entry_line(skill);
        let cost = line.len() + 1;
        if used + cost > budget_bytes {
            dropped += 1;
            continue;
        }
        used += cost;
        lines.push(line);
    }
    if lines.is_empty() {
        return None;
    }
    if dropped > 0 {
        let noun = if dropped == 1 { "skill" } else { "skills" };
        lines.push(format!(
            "- and {dropped} more {noun} omitted (catalog budget reached)."
        ));
    }
    Some(format!(
        "{HEADER}\n\n{SECTION}\n{}\n\n{FOOTER}",
        lines.join("\n")
    ))
}

fn entry_line(skill: &LoadedSkill) -> String {
    match skill.description.as_deref() {
        Some(description) if !description.trim().is_empty() => {
            let one_line = description.split_whitespace().collect::<Vec<_>>().join(" ");
            format!("- {} ({}) - {}", skill.name, skill.id, one_line)
        }
        _ => format!("- {} ({})", skill.name, skill.id),
    }
}

/// Truncate `text` to `budget_bytes` on a character boundary.
///
/// Thin wrapper so callers do not reach into `ark-common` directly and the
/// budget rule stays in one place.
pub fn fit_to_budget(text: &str, budget_bytes: usize) -> &str {
    truncate_utf8(text, budget_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::{SkillSource, SkillTriggers};
    use std::path::PathBuf;

    fn skill(id: &str, description: Option<&str>) -> LoadedSkill {
        LoadedSkill {
            id: id.to_string(),
            name: format!("Name {id}"),
            description: description.map(|value| value.to_string()),
            version: "0.0.0".to_string(),
            root: PathBuf::from("/s").join(id),
            entry_path: PathBuf::from("/s").join(id).join("SKILL.md"),
            entry_text: None,
            triggers: SkillTriggers::default(),
            allowed_tools: Vec::new(),
            priority: 0,
            source: SkillSource::Base,
            legacy: false,
        }
    }

    #[test]
    fn empty_catalog_is_none() {
        assert!(render_catalog(&[], 8_000).is_none());
    }

    #[test]
    fn catalog_lists_ids_and_descriptions() {
        let text = render_catalog(&[skill("pdf", Some("Read PDFs")), skill("x", None)], 8_000)
            .expect("catalog");
        assert!(text.contains("- Name pdf (pdf) - Read PDFs"));
        assert!(text.contains("- Name x (x)"));
        assert!(text.starts_with("## Skills"));
    }

    #[test]
    fn tight_budget_truncates_the_list() {
        let skills: Vec<LoadedSkill> = (0..40)
            .map(|index| skill(&format!("skill-{index}"), Some("a long description here")))
            .collect();
        let text = render_catalog(&skills, 900).expect("catalog");
        assert!(text.contains("omitted (catalog budget reached)"));
        assert!(text.len() < 1_600);
    }

    #[test]
    fn budget_below_the_header_yields_none() {
        assert!(render_catalog(&[skill("pdf", None)], 10).is_none());
    }

    #[test]
    fn fit_to_budget_never_splits_a_codepoint() {
        let text = "你好世界";
        let fitted = fit_to_budget(text, 5);
        assert_eq!(fitted, "你");
        assert!(fitted.len() <= 5);
    }
}
