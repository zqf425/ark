//! Minimal YAML front matter reader for `SKILL.md`.
//!
//! Only the flat subset the convention actually uses is supported: top-level
//! scalars (`id`, `name`, `description`, `version`, `entry`, `priority`), string
//! lists (`allowedTools`), and the two-level `triggers:` block with
//! `commands` / `promptPatterns` / `fileTypes` lists. That is deliberate: the
//! runtime owns no YAML dependency, and unknown keys are ignored (the same
//! leniency the TypeScript reader has), so a `license:` or `metadata:` block
//! cannot make a real skill disappear.

use ark_contract::SkillTriggers;

use ark_common::strip_bom_str;

/// Parsed front matter of one `SKILL.md`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FrontMatter {
    pub id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub entry: Option<String>,
    pub allowed_tools: Vec<String>,
    pub priority: Option<i32>,
    pub triggers: SkillTriggers,
}

/// Parse the `---` delimited block at the top of `text`.
///
/// Returns `None` when the document has no front matter at all, which is not an
/// error: a bare `SKILL.md` falls back to the directory name.
pub fn parse(text: &str) -> Option<FrontMatter> {
    let block = delimited_block(strip_bom_str(text))?;
    Some(parse_block(&block))
}

fn delimited_block(text: &str) -> Option<String> {
    let mut lines: Vec<&str> = Vec::new();
    let mut rest = text.lines();
    let first = rest.next()?;
    if first.trim() != "---" {
        return None;
    }
    for line in rest {
        if line.trim() == "---" {
            return Some(lines.join("\n"));
        }
        lines.push(line);
    }
    // Unterminated block: treat the whole file as malformed front matter and
    // fall back to the directory-name default instead of eating the body.
    None
}

fn parse_block(block: &str) -> FrontMatter {
    let mut front = FrontMatter::default();
    let mut state = BlockState::default();
    for raw in block.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        if let Some(item) = trimmed.strip_prefix("- ") {
            state.push_list_item(item, &mut front);
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = unquote(value.trim());
        if indent == 0 {
            state.on_top_level(key, &value, &mut front);
        } else {
            state.on_nested(key, &value, &mut front);
        }
    }
    front
}

/// Tracks the list that following `- item` lines belong to.
#[derive(Debug, Default)]
struct BlockState {
    in_triggers: bool,
    list_key: Option<ListKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListKey {
    AllowedTools,
    Commands,
    PromptPatterns,
    FileTypes,
}

impl BlockState {
    fn on_top_level(&mut self, key: &str, value: &str, front: &mut FrontMatter) {
        self.in_triggers = false;
        self.list_key = None;
        match key {
            "triggers" => {
                self.in_triggers = true;
            }
            "id" => front.id = non_empty(value),
            "name" => front.name = non_empty(value),
            "description" => front.description = non_empty(value),
            "version" => front.version = non_empty(value),
            "entry" => front.entry = non_empty(value),
            "priority" => front.priority = value.parse::<i32>().ok(),
            "allowedTools" | "allowed_tools" => {
                front.allowed_tools = parse_list(value);
                if front.allowed_tools.is_empty() {
                    self.list_key = Some(ListKey::AllowedTools);
                }
            }
            _ => {}
        }
    }

    fn on_nested(&mut self, key: &str, value: &str, front: &mut FrontMatter) {
        if !self.in_triggers {
            return;
        }
        let target = match key {
            "commands" => ListKey::Commands,
            "promptPatterns" | "prompt_patterns" => ListKey::PromptPatterns,
            "fileTypes" | "file_types" => ListKey::FileTypes,
            _ => return,
        };
        let items = parse_list(value);
        if let Some(bucket) = bucket_mut(&mut front.triggers, target) {
            bucket.extend(items.iter().cloned());
        }
        self.list_key = if items.is_empty() { Some(target) } else { None };
    }

    fn push_list_item(&mut self, item: &str, front: &mut FrontMatter) {
        let Some(key) = self.list_key else {
            return;
        };
        let value = unquote(item.trim());
        if value.is_empty() {
            return;
        }
        match key {
            ListKey::AllowedTools => {
                if !front.allowed_tools.iter().any(|tool| tool == &value) {
                    front.allowed_tools.push(value);
                }
            }
            other => {
                if let Some(bucket) = bucket_mut(&mut front.triggers, other) {
                    bucket.push(value);
                }
            }
        }
    }
}

fn bucket_mut(triggers: &mut SkillTriggers, key: ListKey) -> Option<&mut Vec<String>> {
    match key {
        ListKey::Commands => Some(&mut triggers.commands),
        ListKey::PromptPatterns => Some(&mut triggers.prompt_patterns),
        ListKey::FileTypes => Some(&mut triggers.file_types),
        // `allowedTools` is not a trigger list; the caller handles it before
        // reaching here, and `None` keeps a future key change from silently
        // pushing tool names into `triggers.commands`.
        ListKey::AllowedTools => None,
    }
}

/// Markdown body after the front matter block.
///
/// Used for the `description` fallback, so a bare `SKILL.md` still shows a
/// readable one-line summary in the catalog.
pub fn body(text: &str) -> &str {
    let stripped = strip_bom_str(text);
    let mut offset = 0usize;
    let mut opened = false;
    for line in stripped.split_inclusive('\n') {
        if line.trim() == "---" {
            if opened {
                return &stripped[offset + line.len()..];
            }
            opened = true;
        } else if !opened {
            return stripped;
        }
        offset += line.len();
    }
    stripped
}

/// First non-empty paragraph of a markdown body, without leading `#` marks.
pub fn first_paragraph(markdown: &str) -> Option<String> {
    for block in markdown.split("\n\n") {
        let cleaned = block.trim_start_matches('#').trim();
        if !cleaned.is_empty() {
            return Some(cleaned.to_string());
        }
    }
    None
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// `[a, b]` / `a, b` / `a` -> list of trimmed, unquoted, non-empty items.
fn parse_list(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(trimmed);
    inner
        .split(',')
        .map(|item| unquote(item.trim()))
        .filter(|item| !item.is_empty())
        .map(|item| item.to_string())
        .collect()
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return trimmed[1..trimmed.len() - 1].trim().to_string();
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalars_lists_and_triggers() {
        let text = "---\nid: pdf-tools\nname: \"PDF Tools\"\ndescription: Read PDFs\nversion: 1.2.0\nallowedTools: [read, write]\npriority: 4\ntriggers:\n  commands:\n    - /pdf\n    - $pdf\n  promptPatterns:\n    - pdf\n  fileTypes: [.pdf]\n---\n\n# Body\n";
        let front = parse(text).expect("front matter");
        assert_eq!(front.id.as_deref(), Some("pdf-tools"));
        assert_eq!(front.name.as_deref(), Some("PDF Tools"));
        assert_eq!(front.description.as_deref(), Some("Read PDFs"));
        assert_eq!(front.version.as_deref(), Some("1.2.0"));
        assert_eq!(front.allowed_tools, vec!["read", "write"]);
        assert_eq!(front.priority, Some(4));
        assert_eq!(front.triggers.commands, vec!["/pdf", "$pdf"]);
        assert_eq!(front.triggers.prompt_patterns, vec!["pdf"]);
        assert_eq!(front.triggers.file_types, vec![".pdf"]);
    }

    #[test]
    fn missing_front_matter_is_none() {
        assert!(parse("# Just markdown\n").is_none());
    }

    #[test]
    fn unterminated_front_matter_is_none() {
        assert!(parse("---\nname: x\n\nbody\n").is_none());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let front = parse("---\nname: x\nlicense: MIT\nmetadata:\n  owner: me\n---\n").unwrap();
        assert_eq!(front.name.as_deref(), Some("x"));
    }

    #[test]
    fn body_skips_the_front_matter_block() {
        let text = "---\nname: x\n---\n\n# Title\n\nBody\n";
        assert_eq!(body(text).trim_start(), "# Title\n\nBody\n");
        assert_eq!(body("# No front matter\n"), "# No front matter\n");
    }

    #[test]
    fn first_paragraph_drops_heading_marks() {
        assert_eq!(
            first_paragraph("\n\n# Title\n\nBody").as_deref(),
            Some("Title")
        );
        assert_eq!(first_paragraph("   \n\n"), None);
    }

    #[test]
    fn dash_list_items_stay_inside_their_section() {
        let front =
            parse("---\nname: x\nallowedTools:\n  - read\ntriggers:\n  commands:\n    - /x\n---\n")
                .unwrap();
        assert_eq!(front.allowed_tools, vec!["read"]);
        assert_eq!(front.triggers.commands, vec!["/x"]);
    }
}
