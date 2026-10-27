//! Trigger matching and per-turn activation.
//!
//! The TypeScript runtime scores each trigger source with a fixed weight
//! (explicit mention 1000, command 900, prompt pattern 500, file type 300) and
//! adds `priority`. This module keeps the same ordering intent with the simpler
//! formula the design asks for: `priority * PRIORITY_WEIGHT + matched kinds`,
//! so `priority` dominates and the number of distinct trigger sources breaks
//! ties.
//!
//! ## Deliberate simplification
//!
//! `triggers.promptPatterns` entries are matched as **case-insensitive
//! substrings**, not regular expressions. The runtime owns no regex engine: a
//! pattern is author-supplied configuration, and a malformed or catastrophic
//! pattern must never be able to fail (or stall) skill resolution for every
//! turn of every thread. A pattern written for the TypeScript regex path still
//! matches its own literal text, so real skill packages keep working.

use ark_contract::{SkillActivation, SkillTriggers, SkillTurnResolution};

use crate::domain::skill::LoadedSkill;

/// Weight applied to `priority`, so it outranks the trigger-kind count.
pub const PRIORITY_WEIGHT: i32 = 10;

/// One skill that matched the prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub skill: LoadedSkill,
    pub reason: String,
    pub score: i32,
}

/// Score every skill against `prompt` and return the matches, best first.
pub fn match_skills(skills: &[LoadedSkill], prompt: &str) -> Vec<Match> {
    let lower_prompt = prompt.to_lowercase();
    let prompt_types = file_types_from_prompt(&lower_prompt);
    let mut matches: Vec<Match> = skills
        .iter()
        .filter_map(|skill| match_one(skill, &lower_prompt, &prompt_types))
        .collect();
    matches.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.skill.id.cmp(&b.skill.id))
    });
    matches
}

fn match_one(skill: &LoadedSkill, lower_prompt: &str, prompt_types: &[String]) -> Option<Match> {
    let mut reasons: Vec<String> = Vec::new();
    let triggers = &skill.triggers;
    if let Some(command) = first_command(triggers, lower_prompt) {
        reasons.push(format!("command:{command}"));
    }
    if let Some(pattern) = first_pattern(triggers, lower_prompt) {
        reasons.push(format!("pattern:{pattern}"));
    }
    if let Some(file_type) = first_file_type(triggers, prompt_types) {
        reasons.push(format!("fileType:{file_type}"));
    }
    if reasons.is_empty() {
        return None;
    }
    let kinds = reasons.len() as i32;
    Some(Match {
        score: skill.priority.saturating_mul(PRIORITY_WEIGHT) + kinds,
        reason: reasons.join("+"),
        skill: skill.clone(),
    })
}

fn first_command(triggers: &SkillTriggers, lower_prompt: &str) -> Option<String> {
    triggers
        .commands
        .iter()
        .find(|command| command_matches(command, lower_prompt))
        .cloned()
}

fn first_pattern(triggers: &SkillTriggers, lower_prompt: &str) -> Option<String> {
    triggers
        .prompt_patterns
        .iter()
        .find(|pattern| {
            let needle = pattern.trim().to_lowercase();
            !needle.is_empty() && lower_prompt.contains(&needle)
        })
        .cloned()
}

fn first_file_type(triggers: &SkillTriggers, prompt_types: &[String]) -> Option<String> {
    triggers
        .file_types
        .iter()
        .find(|declared| {
            let normalized = normalize_file_type(declared);
            prompt_types.iter().any(|seen| seen == &normalized)
        })
        .cloned()
}

/// A command trigger matches when the prompt mentions `/cmd`, `$cmd`, or the
/// bare `cmd`, case-insensitively, followed by a word boundary.
fn command_matches(command: &str, lower_prompt: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    let bare = trimmed.trim_start_matches(['/', '$']).trim().to_lowercase();
    if bare.is_empty() {
        return false;
    }
    for needle in [format!("/{bare}"), format!("${bare}")] {
        if contains_word(lower_prompt, &needle) {
            return true;
        }
    }
    false
}

/// `haystack` contains `needle` and the character right after it is not part of
/// a longer word, so `/pdf` does not match `/pdftools`.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let mut from = 0usize;
    while let Some(offset) = haystack[from..].find(needle) {
        let start = from + offset;
        let end = start + needle.len();
        let boundary = haystack[end..]
            .chars()
            .next()
            .map(|next| !next.is_alphanumeric() && next != '-' && next != '_')
            .unwrap_or(true);
        if boundary && haystack.is_char_boundary(start) && haystack.is_char_boundary(end) {
            return true;
        }
        from = end;
    }
    false
}

/// Lowercased `.<ext>` tokens mentioned in the prompt, plus the `*.ext` form.
fn file_types_from_prompt(lower_prompt: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes: Vec<char> = lower_prompt.chars().collect();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != '.' {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric()) {
            end += 1;
        }
        if end > index + 1 {
            let extension: String = bytes[index..end].iter().collect();
            if !out.contains(&extension) {
                out.push(extension);
            }
        }
        index = end.max(index + 1);
    }
    out
}

/// Normalize a declared file type to a leading-dot, lowercase form.
pub fn normalize_file_type(value: &str) -> String {
    let trimmed = value.trim().trim_start_matches('*').to_lowercase();
    if trimmed.is_empty() {
        return trimmed;
    }
    if trimmed.starts_with('.') {
        trimmed
    } else {
        format!(".{trimmed}")
    }
}

/// Build the turn resolution from ranked matches.
///
/// `catalog` is rendered by the caller, which owns the budget and the visible
/// skill set.
pub fn build_resolution(active: &[Match], instructions: Vec<String>) -> SkillTurnResolution {
    let activations: Vec<SkillActivation> = active
        .iter()
        .map(|matched| SkillActivation {
            skill_id: matched.skill.id.clone(),
            reason: matched.reason.clone(),
            score: matched.score,
        })
        .collect();
    let active_skill_ids: Vec<String> = activations
        .iter()
        .map(|activation| activation.skill_id.clone())
        .collect();
    SkillTurnResolution {
        active_skill_ids,
        activations,
        instructions,
        allowed_tool_names: intersect_allowed_tools(active),
        injected_bytes: 0,
        catalog: None,
    }
}

/// Intersection of every `allowedTools` list declared by the active skills.
///
/// Returns `None` when no active skill declares any tool: an empty declaration
/// means "this skill does not restrict tools", so the turn stays unrestricted.
pub fn intersect_allowed_tools(active: &[Match]) -> Option<Vec<String>> {
    let declared: Vec<&Vec<String>> = active
        .iter()
        .map(|matched| &matched.skill.allowed_tools)
        .filter(|tools| !tools.is_empty())
        .collect();
    let (first, rest) = declared.split_first()?;
    let mut allowed: Vec<String> = first.to_vec();
    for tools in rest {
        allowed.retain(|tool| tools.iter().any(|candidate| candidate == tool));
    }
    allowed.sort();
    allowed.dedup();
    if allowed.is_empty() {
        // The declared sets do not overlap. An empty allow-list would block
        // every tool, which is never what an author means; treat it as no
        // restriction and let the loop keep its full registry.
        return None;
    }
    Some(allowed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn skill(id: &str, priority: i32, triggers: SkillTriggers) -> LoadedSkill {
        LoadedSkill {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            version: "0.0.0".to_string(),
            root: PathBuf::from("/skills").join(id),
            entry_path: PathBuf::from("/skills").join(id).join("SKILL.md"),
            entry_text: Some(format!("body of {id}")),
            triggers,
            allowed_tools: Vec::new(),
            priority,
            source: ark_contract::SkillSource::Base,
            legacy: false,
        }
    }

    #[test]
    fn command_triggers_match_slash_and_dollar_forms() {
        let triggers = SkillTriggers {
            commands: vec!["/pdf".to_string()],
            ..Default::default()
        };
        let skills = vec![skill("pdf", 0, triggers)];
        assert_eq!(match_skills(&skills, "/pdf extract").len(), 1);
        assert_eq!(match_skills(&skills, "please run $PDF now").len(), 1);
        assert!(match_skills(&skills, "/pdftools").is_empty());
        assert!(match_skills(&skills, "unrelated").is_empty());
    }

    #[test]
    fn prompt_patterns_match_case_insensitively() {
        let triggers = SkillTriggers {
            prompt_patterns: vec!["Unit Test".to_string()],
            ..Default::default()
        };
        let skills = vec![skill("tests", 0, triggers)];
        let matches = match_skills(&skills, "add a unit TEST please");
        assert_eq!(matches.len(), 1);
        assert!(matches[0].reason.contains("pattern:"));
    }

    #[test]
    fn file_types_match_star_and_plain_extensions() {
        let triggers = SkillTriggers {
            file_types: vec!["pdf".to_string()],
            ..Default::default()
        };
        let skills = vec![skill("pdf", 0, triggers)];
        assert_eq!(match_skills(&skills, "read *.pdf please").len(), 1);
        assert_eq!(match_skills(&skills, "open report.pdf").len(), 1);
        assert!(match_skills(&skills, "open report.docx").is_empty());
    }

    #[test]
    fn higher_priority_wins_and_kinds_break_ties() {
        let one = skill(
            "one",
            5,
            SkillTriggers {
                prompt_patterns: vec!["deploy".to_string()],
                ..Default::default()
            },
        );
        let two = skill(
            "two",
            5,
            SkillTriggers {
                prompt_patterns: vec!["deploy".to_string()],
                file_types: vec![".md".to_string()],
                ..Default::default()
            },
        );
        let ranked = match_skills(&[one, two], "deploy the notes in a.md");
        assert_eq!(ranked[0].skill.id, "two");
        assert_eq!(ranked[1].skill.id, "one");
    }

    #[test]
    fn allowed_tools_intersect_and_collapse_to_none() {
        let mut a = skill("a", 0, SkillTriggers::default());
        a.allowed_tools = vec!["read".to_string(), "write".to_string()];
        let mut b = skill("b", 0, SkillTriggers::default());
        b.allowed_tools = vec!["write".to_string(), "edit".to_string()];
        let active = [
            Match {
                skill: a.clone(),
                reason: "pattern:x".to_string(),
                score: 1,
            },
            Match {
                skill: b.clone(),
                reason: "pattern:x".to_string(),
                score: 0,
            },
        ];
        assert_eq!(
            intersect_allowed_tools(&active),
            Some(vec!["write".to_string()])
        );

        let mut c = skill("c", 0, SkillTriggers::default());
        c.allowed_tools = vec!["grep".to_string()];
        let disjoint = [
            Match {
                skill: a,
                reason: "pattern:x".to_string(),
                score: 1,
            },
            Match {
                skill: c,
                reason: "pattern:x".to_string(),
                score: 1,
            },
        ];
        assert_eq!(intersect_allowed_tools(&disjoint), None);

        let undeclared = [Match {
            skill: skill("d", 0, SkillTriggers::default()),
            reason: "pattern:x".to_string(),
            score: 1,
        }];
        assert_eq!(intersect_allowed_tools(&undeclared), None);
    }

    #[test]
    fn file_type_normalization_adds_the_dot() {
        assert_eq!(normalize_file_type("PDF"), ".pdf");
        assert_eq!(normalize_file_type("*.rs"), ".rs");
        assert_eq!(normalize_file_type(" .md "), ".md");
    }
}
