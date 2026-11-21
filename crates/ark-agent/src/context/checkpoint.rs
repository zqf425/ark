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
/// How many of the oldest `[user requests]` lines survive when the section
/// overflows its quota. The earliest request carries the thread's original
/// goal; dropping it silently is what left the model in session cb274b9d
/// without its task anchor after the second compaction.
const KEEP_OLDEST_USER_LINES: usize = 6;

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
    ///
    /// A previously rendered checkpoint inside the folded region is chained:
    /// its sections are absorbed into this one instead of being re-parsed line
    /// by line. Plain text user requests never match the structured-line
    /// prefixes, so without chaining every re-compaction silently dropped all
    /// earlier user requests — the original goal included.
    pub fn from_messages(messages: &[ModelMessage]) -> Self {
        let mut out = Self::default();
        for message in messages {
            let text = message.content.as_str();
            if is_generated_checkpoint(message) {
                out.absorb_checkpoint_text(text);
                continue;
            }
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
    ///
    /// Failure evidence needs section awareness: a real error line often drags
    /// the source-code lines that follow it into the section, and in session
    /// cb274b9d those lines consumed most of the `[failed attempts]` quota.
    /// Code-shaped lines only attach while the previous failure line was itself
    /// a genuine diagnostic, so a stack trace keeps its frames but `Boolean(`
    /// fragments dropped by a formatter never enter the note.
    pub fn from_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> Self {
        let mut out = Self::default();
        for text in texts {
            let mut failure_armed = false;
            for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                if is_structured_line(line) {
                    push_unique(&mut out.entries, line.to_owned());
                }
                if is_failure_line(line) {
                    push_failure(&mut out.failures, line, MAX_FAILURE_LINE_CHARS);
                    failure_armed = true;
                } else if failure_armed {
                    // Not a failure and not structured: a code continuation.
                    // Disarm so unrelated later lines are not captured either.
                    failure_armed = false;
                }
                for identifier in identifiers_in(line) {
                    push_unique(&mut out.identifiers, identifier);
                }
            }
        }
        out
    }

    /// Absorb the sections of a previously rendered checkpoint.
    ///
    /// Section titles themselves are not re-emitted (the render pass writes
    /// fresh ones), and bookkeeping lines such as `[N earlier messages
    /// compacted]` are dropped. Absorbed lines keep the oldest-first order of
    /// the prior note, so user requests read as a chronological chain.
    fn absorb_checkpoint_text(&mut self, text: &str) {
        let mut section = CheckpointSection::None;
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            match line {
                "[user requests]" => section = CheckpointSection::UserRequests,
                "[goals / constraints / decisions]" => {
                    section = CheckpointSection::Entries;
                }
                "[failed attempts - do not repeat]" => section = CheckpointSection::Failures,
                "[remembered identifiers]" => section = CheckpointSection::Identifiers,
                _ if line.starts_with('[') && line.ends_with(']') => {
                    section = CheckpointSection::None;
                }
                _ => {
                    // Structured facts (`Goal:`, plan checkbox rows) may sit
                    // under any section of an older render; they always belong
                    // to entries regardless of where they were displayed.
                    if is_structured_line(line) {
                        push_unique(&mut self.entries, line.to_owned());
                        continue;
                    }
                    let content = line.strip_prefix("- ");
                    match (section, content) {
                        (CheckpointSection::UserRequests, Some(content)) => {
                            push_bounded(
                                &mut self.user_lines,
                                content.to_owned(),
                                MAX_USER_LINE_CHARS,
                            );
                        }
                        (CheckpointSection::Entries, Some(content)) => {
                            push_unique(&mut self.entries, content.to_owned());
                        }
                        (CheckpointSection::Failures, Some(content)) => {
                            push_failure(&mut self.failures, content, MAX_FAILURE_LINE_CHARS);
                        }
                        (CheckpointSection::Identifiers, Some(content)) => {
                            push_unique(&mut self.identifiers, content.to_owned());
                        }
                        _ => {}
                    }
                }
            }
        }
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
            push_failure(&mut self.failures, &line, MAX_FAILURE_LINE_CHARS);
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
                self.bounded_user_lines()
                    .into_iter()
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
    ///
    /// User requests come first: the earliest request carries the original
    /// goal, and in session cb274b9d the budget cut silently dropped it while
    /// keeping noise such as `Base64`. Identifiers rank last because they are
    /// lossless fragments — anything valuable in them also appears in the
    /// structured sections.
    fn prioritized_entries(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.extend(
            self.bounded_user_lines()
                .into_iter()
                .map(|line| format!("- {line}")),
        );
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
        lines
    }

    /// User requests capped at the section quota, keeping both ends.
    ///
    /// The oldest lines survive verbatim (the goal chain) and the newest fill
    /// the rest (current steering); the middle is the part that is safe to
    /// fold, and the renderer says so explicitly.
    fn bounded_user_lines(&self) -> Vec<String> {
        if self.user_lines.len() <= MAX_SECTION_LINES {
            return self.user_lines.clone();
        }
        let newest = MAX_SECTION_LINES - KEEP_OLDEST_USER_LINES;
        let oldest: Vec<String> = self.user_lines[..KEEP_OLDEST_USER_LINES].to_vec();
        let recent: Vec<String> = self.user_lines[self.user_lines.len() - newest..].to_vec();
        let folded = self.user_lines.len() - KEEP_OLDEST_USER_LINES - newest;
        let mut lines = oldest;
        lines.push(format!(
            "[{folded} middle user requests folded - see the event log via thread_events]"
        ));
        lines.extend(recent);
        lines
    }
}

use crate::context::TokenEstimator;

/// Which `[section]` a prior checkpoint's lines belong to while absorbing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckpointSection {
    None,
    UserRequests,
    Entries,
    Failures,
    Identifiers,
}

fn push_unique(vec: &mut Vec<String>, value: String) {
    if !vec.contains(&value) {
        vec.push(value);
    }
}

/// Append one failure line, collapsing numeric variants of an entry already
/// present instead of spending a new quota slot on them. Counting misses like
/// `actionCount=27, actual 28` re-fire per module and per rerun; session
/// cb274b9d filled its `[failed attempts]` section with near-duplicates.
fn push_failure(vec: &mut Vec<String>, line: &str, max_chars: usize) {
    if line.chars().count() > max_chars {
        return;
    }
    if let Some(existing) = vec
        .iter_mut()
        .find(|existing| normalize_failure(existing) == normalize_failure(line))
    {
        bump_failure_count(existing);
        return;
    }
    vec.push(line.to_owned());
}

/// Collapse every digit run to a placeholder so numeric variants compare equal.
fn normalize_failure(line: &str) -> String {
    let mut normalized = String::with_capacity(line.len());
    let mut digits = false;
    for character in line.chars() {
        if character.is_ascii_digit() {
            digits = true;
        } else {
            if digits {
                normalized.push('\u{0}');
                digits = false;
            }
            normalized.push(character);
        }
    }
    if digits {
        normalized.push('\u{0}');
    }
    normalized
}

/// Bump or add the trailing `[xN]` counter on a collapsed failure entry.
fn bump_failure_count(entry: &mut String) {
    if let Some(start) = entry.rfind("[x") {
        if entry.ends_with(']') {
            if let Ok(count) = entry[start + 2..entry.len() - 1].parse::<u32>() {
                *entry = format!("{}[x{}]", &entry[..start], count + 1);
                return;
            }
        }
    }
    entry.push_str(" [x2]");
}

fn push_bounded(vec: &mut Vec<String>, value: String, max_chars: usize) {
    if value.chars().count() > max_chars || vec.contains(&value) {
        return;
    }
    vec.push(value);
}

fn is_structured_line(line: &str) -> bool {
    // A plan-seed todo line (`- [ ] title`, from `plan_seed_message` /
    // `todo_write`) carries the durable plan across a compaction boundary:
    // the seed tail is restated every turn, but the checkpoint must also name
    // it so an in-turn compact does not drop the objective list.
    if line.starts_with("- [ ]") || line.starts_with("- [~]") || line.starts_with("- [x]") {
        return true;
    }
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

/// A real diagnostic, not a source-code line that happens to start like one.
///
/// Session cb274b9d filled the `[failed attempts]` section with formatter
/// output such as `failed: Boolean(failed),` and `errorMessage: null,` —
/// minified JS that starts with the same words. Two filters keep them out:
/// a word boundary after the keyword (the `lower.starts_with(prefix)`
/// variant that would wrongly accept `failed:` is replaced by a
/// next-char check), and a code-shape rejection for lines that carry
/// JavaScript/call syntax.
fn is_failure_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if is_code_shaped(line) {
        return false;
    }
    let keyword_prefixes = ["error", "failed", "failure"];
    keyword_prefixes.iter().any(|prefix| {
        lower.strip_prefix(prefix).is_some_and(|rest| {
            rest.is_empty() || !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
        })
    }) || lower.contains("panicked at")
        || lower.contains("stack trace")
        || lower.contains("command not found")
        || lower.contains("test failed")
}

/// Line looks like source code rather than a human- or tool-written diagnostic.
///
/// Conservative by design: a genuine error message containing a snippet (for
/// example a path with parentheses) must stay. The patterns below only match
/// the shapes the observed noise took — trailing separators, arrow bodies,
/// `instanceof`/ternary chains, and keyword-parameter lines.
fn is_code_shaped(line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed.ends_with(';')
        || trimmed.ends_with("','")
        || trimmed.ends_with(");")
        || trimmed.ends_with(',') && trimmed.contains('(')
        || trimmed.contains("instanceof")
        || trimmed.contains(" => ")
        || trimmed.starts_with("errorMessage:")
        || trimmed.starts_with("failed:")
        || trimmed.starts_with("failure:")
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
                && value
                    .chars()
                    .any(|character: char| character.is_ascii_digit())))
}
