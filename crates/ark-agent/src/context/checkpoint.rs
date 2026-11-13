use ark_ports::{ModelMessage, ModelRole};

/// Prefix every generated checkpoint starts with. `ModelMessage` carries no
/// marker field, so identity at the in-memory layer is this prefix *combined
/// with* `ModelRole::System`; replay maps user items to `User`, so user text can
/// never masquerade as a checkpoint. Persisted identity is the structured
/// `ItemMarker::ContextCheckpoint`, never this string.
pub(crate) const CHECKPOINT_PREFIX: &str = "[context checkpoint";

const MAX_USER_LINE_CHARS: usize = 240;
const MAX_FAILURE_LINE_CHARS: usize = 240;
const MAX_SECTION_LINES: usize = 40;

/// Deterministic, model-readable handoff retained across a compaction boundary.
///
/// Recall-first on purpose: user requests are kept verbatim, failure evidence is
/// kept so the next model does not repeat a rejected attempt, and structured
/// facts plus short identifiers are extracted. Only bulky tool output is
/// dropped — and it stays recoverable from the append-only event log.
///
/// A model-written prose summary can later enrich these sections; this
/// deterministic extraction remains the fallback when that call is unavailable,
/// so compaction never depends on a second model round.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextCheckpoint {
    /// User requests, verbatim, deduplicated, bounded per line.
    user_lines: Vec<String>,
    /// Structured facts: Goal / Constraint / Decision / Next / Reference / etc.
    entries: Vec<String>,
    /// Short identifiers (quoted values, build ids, `key=value`).
    identifiers: Vec<String>,
    /// Failure evidence: errors, rejected attempts, stack-trace lines.
    failures: Vec<String>,
}

impl ContextCheckpoint {
    /// High-recall extraction from the model-visible history.
    ///
    /// Unlike earlier versions this reads *every* message, including the recent
    /// tail: a checkpoint is a durable boundary, and the next turn replays only
    /// the checkpoint plus newer rows, so the tail must be represented here
    /// rather than assumed to survive verbatim.
    pub fn from_messages(messages: &[ModelMessage]) -> Self {
        let mut out = Self::default();
        for message in messages {
            let text = message.content.as_str();
            out.merge(Self::from_texts([text]));
            match message.role {
                ModelRole::User => {
                    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                        push_bounded(&mut out.user_lines, line.to_owned(), MAX_USER_LINE_CHARS);
                    }
                }
                ModelRole::Tool => {
                    for line in text.lines().map(str::trim) {
                        if is_failure_line(line) {
                            push_bounded(
                                &mut out.failures,
                                line.to_owned(),
                                MAX_FAILURE_LINE_CHARS,
                            );
                        }
                    }
                }
                _ => {}
            }
            for identifier in identifiers_in(text) {
                push_unique(&mut out.identifiers, identifier);
            }
        }
        out
    }

    /// Build structured facts from durable text rows. Also used by the manual
    /// `/compact` route, which has no role information.
    pub fn from_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> Self {
        let mut out = Self::default();
        for text in texts {
            for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                if is_structured_line(line) {
                    push_unique(&mut out.entries, line.to_owned());
                }
                if is_failure_line(line) {
                    push_bounded(&mut out.failures, line.to_owned(), MAX_FAILURE_LINE_CHARS);
                }
                for identifier in identifiers_in(line) {
                    push_unique(&mut out.identifiers, identifier);
                }
            }
        }
        out
    }

    fn merge(&mut self, other: Self) {
        for entry in other.entries {
            push_unique(&mut self.entries, entry);
        }
        for identifier in other.identifiers {
            push_unique(&mut self.identifiers, identifier);
        }
        for line in other.user_lines {
            push_bounded(&mut self.user_lines, line, MAX_USER_LINE_CHARS);
        }
        for line in other.failures {
            push_bounded(&mut self.failures, line, MAX_FAILURE_LINE_CHARS);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
            && self.identifiers.is_empty()
            && self.user_lines.is_empty()
            && self.failures.is_empty()
    }

    /// Render the durable handoff for the next model context window.
    pub fn render(&self) -> String {
        let mut lines = vec![
            format!("{CHECKPOINT_PREFIX}]"),
            "[handoff] Another model will resume from this summary. Build on it; \
             the folded messages remain in the authoritative event log and can be \
             retrieved with thread_events / thread_export, or restored with thread_rewind."
                .to_string(),
        ];
        if self.is_empty() {
            lines.push("[no structured facts extracted]".to_string());
            return lines.join("\n");
        }
        if !self.user_lines.is_empty() {
            lines.push("[user requests]".to_string());
            lines.extend(
                self.user_lines
                    .iter()
                    .take(MAX_SECTION_LINES)
                    .map(|line| format!("- {line}")),
            );
        }
        if !self.entries.is_empty() {
            lines.push("[goals / constraints / decisions]".to_string());
            lines.extend(self.entries.iter().take(MAX_SECTION_LINES).cloned());
        }
        if !self.failures.is_empty() {
            lines.push("[failed attempts - do not repeat]".to_string());
            lines.extend(
                self.failures
                    .iter()
                    .take(MAX_SECTION_LINES)
                    .map(|line| format!("- {line}")),
            );
        }
        if !self.identifiers.is_empty() {
            lines.push("[remembered identifiers]".to_string());
            lines.extend(
                self.identifiers
                    .iter()
                    .take(MAX_SECTION_LINES)
                    .map(|identifier| format!("- {identifier}")),
            );
        }
        lines.join("\n")
    }

    /// Render without exceeding the configured context budget, keeping the
    /// highest-signal lines first and telling the model what was dropped.
    pub fn render_with_token_limit(&self, max_tokens: u64) -> String {
        let rendered = self.render();
        if TokenEstimator::new().estimate_text(&rendered) <= max_tokens {
            return rendered;
        }

        let mut limited = format!("{CHECKPOINT_PREFIX}: truncated]");
        for line in self.prioritized_entries() {
            let candidate = format!("{limited}\n{line}");
            if TokenEstimator::new().estimate_text(&candidate) > max_tokens {
                continue;
            }
            limited = candidate;
        }
        limited
    }

    /// Highest-signal lines first for the budget-limited render.
    fn prioritized_entries(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.extend(self.entries.iter().take(MAX_SECTION_LINES).cloned());
        lines.extend(
            self.failures
                .iter()
                .take(MAX_SECTION_LINES)
                .map(|line| format!("- {line}")),
        );
        lines.extend(
            self.identifiers
                .iter()
                .take(MAX_SECTION_LINES)
                .map(|identifier| format!("- {identifier}")),
        );
        lines.extend(
            self.user_lines
                .iter()
                .take(MAX_SECTION_LINES)
                .map(|line| format!("- {line}")),
        );
        lines
    }
}

use crate::context::TokenEstimator;

fn push_unique(vec: &mut Vec<String>, value: String) {
    if !vec.contains(&value) {
        vec.push(value);
    }
}

fn push_bounded(vec: &mut Vec<String>, value: String, max_chars: usize) {
    if value.chars().count() > max_chars || vec.contains(&value) {
        return;
    }
    vec.push(value);
}

fn is_structured_line(line: &str) -> bool {
    [
        "Goal:",
        "Constraint:",
        "Decision:",
        "Next:",
        "Progress:",
        "Reference:",
        "File:",
        "Path:",
        "Todo:",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
}

fn is_failure_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("error")
        || lower.starts_with("failed")
        || lower.starts_with("failure")
        || lower.contains("panicked at")
        || lower.contains("stack trace")
        || lower.contains("command not found")
        || lower.contains("test failed")
}

fn identifiers_in(text: &str) -> Vec<String> {
    let mut identifiers = Vec::new();
    let mut quoted = None;
    for (index, character) in text.char_indices() {
        match (quoted, character) {
            (None, '"' | '\'') => quoted = Some((character, index + character.len_utf8())),
            (Some((quote, start)), character) if character == quote => {
                let value = text[start..index].trim();
                if is_short_quoted_value(value) {
                    identifiers.push(value.to_owned());
                }
                quoted = None;
            }
            _ => {}
        }
    }
    identifiers.extend(
        text.split_whitespace()
            .map(|word| word.trim_matches(|character: char| !is_identifier_character(character)))
            .filter(|word| is_short_identifier(word))
            .map(ToOwned::to_owned),
    );
    identifiers
        .into_iter()
        .fold(Vec::new(), |mut unique, value| {
            if !unique.contains(&value) {
                unique.push(value);
            }
            unique
        })
}

pub(crate) fn is_generated_checkpoint(message: &ModelMessage) -> bool {
    message.role == ModelRole::System && message.content.starts_with(CHECKPOINT_PREFIX)
}

fn is_short_quoted_value(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 80 && !value.chars().any(char::is_control)
}

fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '=')
}

fn is_short_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value.chars().all(is_identifier_character)
        && ((value.contains('=') && !value.starts_with('=') && !value.ends_with('='))
            || (value
                .chars()
                .any(|character: char| character.is_ascii_uppercase())
                && value.chars().any(|character: char| character.is_ascii_digit())))
}
