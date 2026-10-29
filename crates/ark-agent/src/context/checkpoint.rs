use ark_ports::{ModelMessage, ModelRole};

/// Deterministic, model-readable state retained across a compaction boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextCheckpoint {
    entries: Vec<String>,
    identifiers: Vec<String>,
}

impl ContextCheckpoint {
    /// Preserve user goals and constraints plus assistant decisions verbatim.
    pub fn from_messages(messages: &[ModelMessage]) -> Self {
        let mut checkpoint = Self::from_texts(
            messages
                .iter()
                .filter(|message| message.role != ModelRole::System)
                .map(|message| message.content.as_str()),
        );
        for message in messages
            .iter()
            .filter(|message| is_generated_checkpoint(message))
        {
            checkpoint.merge(Self::from_texts([message.content.as_str()]));
        }
        checkpoint.identifiers = messages
            .iter()
            .filter(|message| message.role == ModelRole::User || is_generated_checkpoint(message))
            .flat_map(|message| identifiers_in(&message.content))
            .fold(Vec::new(), |mut identifiers, identifier| {
                if !identifiers.contains(&identifier) {
                    identifiers.push(identifier);
                }
                identifiers
            });
        checkpoint
    }

    /// Build a deterministic checkpoint from durable text rows.
    pub fn from_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> Self {
        let texts: Vec<&str> = texts.into_iter().map(str::trim).collect();
        let entries = texts
            .iter()
            .copied()
            .flat_map(str::lines)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .filter(|text| {
                text.starts_with("Goal:")
                    || text.starts_with("Constraint:")
                    || text.starts_with("Decision:")
            })
            .map(ToOwned::to_owned)
            .fold(Vec::new(), |mut entries, entry| {
                if !entries.contains(&entry) {
                    entries.push(entry);
                }
                entries
            });
        let mut identifiers = Vec::new();
        for text in texts {
            for identifier in identifiers_in(text) {
                if !identifiers.contains(&identifier) {
                    identifiers.push(identifier);
                }
            }
        }
        Self {
            entries,
            identifiers,
        }
    }

    fn merge(&mut self, other: Self) {
        for entry in other.entries {
            if !self.entries.contains(&entry) {
                self.entries.push(entry);
            }
        }
        for identifier in other.identifiers {
            if !self.identifiers.contains(&identifier) {
                self.identifiers.push(identifier);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.identifiers.is_empty()
    }

    /// Render the durable state for the next model context window.
    pub fn render(&self) -> String {
        if self.is_empty() {
            "[context checkpoint: no structured facts extracted]".to_string()
        } else {
            self.render_entries()
        }
    }

    /// Render a checkpoint without exceeding the configured context budget.
    pub fn render_with_token_limit(&self, max_tokens: u64) -> String {
        let rendered = self.render();
        if TokenEstimator::new().estimate_text(&rendered) <= max_tokens {
            return rendered;
        }

        let mut limited = "[context checkpoint: truncated]".to_string();
        for entry in self.prioritized_entries() {
            let candidate = format!("{limited}\n{entry}");
            if TokenEstimator::new().estimate_text(&candidate) > max_tokens {
                continue;
            }
            limited = candidate;
        }
        limited
    }

    fn render_entries(&self) -> String {
        let mut lines = vec!["[context checkpoint]".to_string()];
        if !self.identifiers.is_empty() {
            lines.push("[remembered identifiers]".to_string());
            lines.extend(
                self.identifiers
                    .iter()
                    .map(|identifier| format!("- {identifier}")),
            );
        }
        lines.extend(self.entries.iter().cloned());
        lines.join("\n")
    }

    fn prioritized_entries(&self) -> Vec<String> {
        let mut entries = Vec::new();
        if !self.identifiers.is_empty() {
            entries.push("[remembered identifiers]".to_string());
            entries.extend(
                self.identifiers
                    .iter()
                    .map(|identifier| format!("- {identifier}")),
            );
        }
        entries.extend(self.entries.iter().cloned());
        entries
    }
}

use crate::context::TokenEstimator;

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

fn is_generated_checkpoint(message: &ModelMessage) -> bool {
    message.role == ModelRole::System && message.content.starts_with("[context checkpoint")
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
                .any(|character| character.is_ascii_uppercase())
                && value.chars().any(|character| character.is_ascii_digit())))
}
