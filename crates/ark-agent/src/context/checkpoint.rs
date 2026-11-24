use ark_ports::{ModelMessage, ModelRole, ToolCallRef};

/// Prefix every generated checkpoint starts with. `ModelMessage` carries no
/// marker field, so identity at the in-memory layer is this prefix *combined
/// with* `ModelRole::System`; replay maps user items to `User`, so user text can
/// never masquerade as a checkpoint. Persisted identity is the structured
/// `ItemMarker::ContextCheckpoint`, never this string.
pub(crate) const CHECKPOINT_PREFIX: &str = "[context checkpoint";

const MAX_USER_LINE_CHARS: usize = 240;
const MAX_FAILURE_LINE_CHARS: usize = 240;
const MAX_TOOL_EVIDENCE_CHARS: usize = 240;
const MAX_SECTION_LINES: usize = 40;
/// How many of the oldest `[user requests]` lines survive when the section
/// overflows its quota. The earliest request carries the thread's original
/// goal; dropping it silently is what left the model in session cb274b9d
/// without its task anchor after the second compaction.
const KEEP_OLDEST_USER_LINES: usize = 6;
/// Independent cap for the `[thread plan]` tail restate. Plan rows do not
/// compete with the render sections' quota: the restate is the model's
/// objective anchor and must not be crowded out by decision rows (review N1).
/// The truncation-notice test asserts against this name.
pub(crate) const MAX_PLAN_LINES: usize = 40;

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
    /// Bounded typed evidence from tool calls and their results.
    tool_evidence: Vec<String>,
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
            if let ModelRole::User = message.role {
                for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                    push_bounded(&mut out.user_lines, line.to_owned(), MAX_USER_LINE_CHARS);
                }
            }
            // Tool failure rows need no separate branch: `from_texts` already
            // runs the same `is_failure_line` filter and `push_failure` merge
            // over every line. A dedicated Tool arm processed the same text a
            // second time and double-counted every tool error (rendered
            // `(repeated x4)` for two occurrences, review S2's root cause).
            for call in &message.tool_calls {
                push_bounded(
                    &mut out.tool_evidence,
                    tool_call_evidence(call),
                    MAX_TOOL_EVIDENCE_CHARS,
                );
            }
            if message.role == ModelRole::Tool {
                if let Some(evidence) = tool_result_evidence(message) {
                    push_bounded(&mut out.tool_evidence, evidence, MAX_TOOL_EVIDENCE_CHARS);
                }
            }
            for identifier in identifiers_in(text) {
                push_unique(&mut out.identifiers, identifier);
            }
        }
        out
    }

    /// Extract a chained checkpoint without recounting the prior retained tail.
    ///
    /// A checkpoint is persisted after the current tail, so that tail must be
    /// represented in the checkpoint even though it remains verbatim in memory.
    /// The rendered retained-tail count and fingerprint let the next boundary
    /// skip those same raw rows only when they are actually present. A replayed
    /// turn starts after the checkpoint, so its new rows must never be skipped.
    pub fn from_compaction_history(messages: &[ModelMessage]) -> Self {
        let Some(index) = messages.iter().rposition(is_generated_checkpoint) else {
            return Self::from_messages(messages);
        };
        let retained = checkpoint_retained_tail(messages[index].content.as_str());
        let suffix = &messages[index + 1..];
        let retained_rows: Vec<&ModelMessage> = suffix
            .iter()
            .filter(|message| message.role != ModelRole::System)
            .take(retained.map_or(0, |(count, _)| count))
            .collect();
        let skip = retained.filter(|(count, fingerprint)| {
            retained_rows.len() == *count
                && retained_tail_fingerprint(retained_rows.iter().copied()) == *fingerprint
        });
        let mut source = vec![messages[index].clone()];
        let mut tail_rows = 0usize;
        for message in suffix {
            if message.role != ModelRole::System {
                tail_rows += 1;
                if skip.is_some_and(|(count, _)| tail_rows <= count) {
                    continue;
                }
            }
            source.push(message.clone());
        }
        Self::from_messages(&source)
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
    /// fresh ones), bookkeeping lines (`[N earlier messages compacted]`, the
    /// `[N middle user requests folded]` marker) are dropped rather than being
    /// mistaken for real content (review M2), and failure lines pass the same
    /// diagnostic filter as fresh captures so the two channels stay symmetric
    /// (review B1). Absorbed lines keep the oldest-first order of the prior
    /// note, so user requests read as a chronological chain.
    fn absorb_checkpoint_text(&mut self, text: &str) {
        let mut section = CheckpointSection::None;
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            match line {
                "[user requests]" => section = CheckpointSection::UserRequests,
                "[goals / constraints / decisions]" => {
                    section = CheckpointSection::Entries;
                }
                "[failed attempts - do not repeat]" => section = CheckpointSection::Failures,
                "[tool evidence]" => section = CheckpointSection::ToolEvidence,
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
                            // A folded marker bullet survived into the prior
                            // note (rendered before M2 was fixed): drop it so
                            // it never occupies a user-request slot again.
                            if content.starts_with('[') && content.ends_with(']') {
                                continue;
                            }
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
                            // Absorption must never bump a count: an older
                            // checkpoint already subsumes the occurrences of
                            // the rows it folded. Re-reading the full journal
                            // (manual /compact) would otherwise inflate the
                            // count once per compaction (review S3). Fresh
                            // rows after the checkpoint still bump via
                            // `from_texts`, which is the recurrence signal.
                            if is_failure_line(content)
                                && !self.failures.iter().any(|existing| {
                                    failure_match_key(existing) == failure_match_key(content)
                                })
                            {
                                push_failure(&mut self.failures, content, MAX_FAILURE_LINE_CHARS);
                            }
                        }
                        (CheckpointSection::ToolEvidence, Some(content)) => {
                            push_bounded(
                                &mut self.tool_evidence,
                                content.to_owned(),
                                MAX_TOOL_EVIDENCE_CHARS,
                            );
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
        for line in other.tool_evidence {
            push_bounded(&mut self.tool_evidence, line, MAX_TOOL_EVIDENCE_CHARS);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
            && self.identifiers.is_empty()
            && self.user_lines.is_empty()
            && self.failures.is_empty()
            && self.tool_evidence.is_empty()
    }

    /// Extract the durable plan rows (the `Goal:` line and the `- [ ]`/`- [~]`/`- [x]`
    /// checkbox lines the turn seed restates every round) from the extracted
    /// facts, rendered as a standalone `[thread plan]` block.
    ///
    /// Plan rows are filtered from the *whole* `entries` list before any
    /// quota: they sit at the seed tail, after earlier structured facts, so a
    /// take-first scan would drop them exactly in the longest turns where the
    /// restate matters (review N1). The same todo appears once per rendered
    /// status (`- [ ] run tests` before completion, `- [x] run tests`
    /// after), and successive objectives update the one `Goal:` slot, so the
    /// fold is last-wins per todo title — restating both statuses or an
    /// outdated objective would show the model a self-contradicting plan
    /// (review N2). Plan lines are capped by their own quota, independent of
    /// the render section quota: they do not compete with decision rows.
    ///
    /// Returns `None` when the folded region carried no plan rows.
    pub(crate) fn plan_restate(&self) -> Option<String> {
        let plan_lines: Vec<&String> = self
            .entries
            .iter()
            .filter(|entry| entry.starts_with("Goal:") || is_plan_checkbox_line(entry))
            .collect();
        if plan_lines.is_empty() {
            return None;
        }
        // First pass: collapse status variants / objective updates last-wins
        // per title, keeping insertion order of first appearance.
        let mut ordered: Vec<String> = Vec::new();
        let mut slots: Vec<(String, usize)> = Vec::new();
        for line in plan_lines {
            let title = todo_title(line);
            match slots.iter_mut().find(|(existing, _)| *existing == title) {
                Some((_, slot)) => {
                    ordered[*slot] = line.clone();
                }
                None => {
                    slots.push((title, ordered.len()));
                    ordered.push(line.clone());
                }
            }
        }
        // Second pass: emit the newest objective first (an early break below
        // must never drop it — review M5), then the todo rows in first-
        // appearance order, capped by MAX_PLAN_LINES with an explicit
        // truncation notice (the checkpoint budget render sets the same
        // precedent).
        let newest_goal = ordered
            .iter()
            .rev()
            .find(|line| line.starts_with("Goal:"))
            .cloned();
        let mut lines: Vec<String> = Vec::new();
        if let Some(goal) = newest_goal {
            lines.push(goal);
        }
        for line in &ordered {
            if line.starts_with("Goal:") {
                continue;
            }
            if lines.len() >= MAX_PLAN_LINES {
                lines.push(format!(
                    "[thread plan truncated at {MAX_PLAN_LINES} todo rows]"
                ));
                break;
            }
            lines.push(line.clone());
        }
        if lines.is_empty() {
            return None;
        }
        Some(format!("[thread plan]\n{}", lines.join("\n")))
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
        if !self.tool_evidence.is_empty() {
            lines.push("[tool evidence]".to_string());
            lines.extend(
                self.tool_evidence
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
    ///
    /// Every kept line is prefixed with the section title it belongs to: a
    /// truncated render loses the bare section headers that group the lines,
    /// and a later compaction absorbing this note would then see unassigned
    /// bullets and silently drop everything (review M1). `absorb_checkpoint_text`
    /// re-associates lines with the nearest preceding title, so re-emitting
    /// titles keeps the absorption chain intact across truncated generations.
    pub fn render_with_token_limit(&self, max_tokens: u64) -> String {
        let rendered = self.render();
        if TokenEstimator::new().estimate_text(&rendered) <= max_tokens {
            return rendered;
        }

        let mut limited = format!("{CHECKPOINT_PREFIX}: truncated]");
        let mut emitted_title = "";
        for (section, line) in self.prioritized_entries_with_sections() {
            // The section title only counts as emitted when its line actually
            // fits: advancing the pointer before the fit check would leave a
            // skipped section's later (shorter) lines dangling under the
            // previous title, and absorption would then reassign them to the
            // wrong section (recheck N3).
            let needs_title = section != emitted_title;
            let candidate = if needs_title {
                format!("{limited}\n{section}\n{line}")
            } else {
                format!("{limited}\n{line}")
            };
            if TokenEstimator::new().estimate_text(&candidate) > max_tokens {
                continue;
            }
            limited = candidate;
            if needs_title {
                emitted_title = section;
            }
        }
        limited
    }

    /// [`Self::prioritized_entries`] with the owning section attached per line,
    /// so the budget render can re-emit section titles (M1).
    fn prioritized_entries_with_sections(&self) -> Vec<(&'static str, String)> {
        const USER: &str = "[user requests]";
        const ENTRIES: &str = "[goals / constraints / decisions]";
        const FAILURES: &str = "[failed attempts - do not repeat]";
        const TOOL_EVIDENCE: &str = "[tool evidence]";
        const IDENTIFIERS: &str = "[remembered identifiers]";
        let mut lines = Vec::new();
        for line in self.bounded_user_lines() {
            lines.push((USER, format!("- {line}")));
        }
        for entry in self.entries.iter().take(MAX_SECTION_LINES) {
            lines.push((ENTRIES, entry.clone()));
        }
        for failure in self.failures.iter().take(MAX_SECTION_LINES) {
            lines.push((FAILURES, format!("- {failure}")));
        }
        for evidence in self.tool_evidence.iter().take(MAX_SECTION_LINES) {
            lines.push((TOOL_EVIDENCE, format!("- {evidence}")));
        }
        for identifier in self.identifiers.iter().take(MAX_SECTION_LINES) {
            lines.push((IDENTIFIERS, format!("- {identifier}")));
        }
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
    ToolEvidence,
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
///
/// The match key strips any `[xN]` counter first: the same diagnostic arriving
/// through a different channel (tool result vs. an absorbed checkpoint whose
/// entry was already bumped) must still merge, or the counter semantics break
/// down into duplicate entries (review findings S2/S3).
fn push_failure(vec: &mut Vec<String>, line: &str, max_chars: usize) {
    if line.chars().count() > max_chars {
        return;
    }
    if let Some(existing) = vec
        .iter_mut()
        .find(|existing| failure_match_key(existing) == failure_match_key(line))
    {
        bump_failure_count(existing);
        return;
    }
    vec.push(line.to_owned());
}

/// Collapse digit runs to a placeholder so numeric variants compare equal —
/// but only in counting contexts (`count=`, `actual 28`, `got 27`). Semantic
/// numbers elsewhere (error codes `A3`/`A4`, HTTP statuses `401`/`500`, asset
/// versions `v2`/`v3`) must stay distinct: they are exactly what tells the
/// next model which case not to repeat (review M3). The trailing `[xN]`
/// counter is stripped first so entries bumped in an earlier generation still
/// match their bare form arriving from another channel (review S2/S3).
fn failure_match_key(line: &str) -> String {
    let (bare, _) = strip_failure_counter(line);
    let lower = bare.to_ascii_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let mut normalized = String::with_capacity(lower.len());
    let mut index = 0usize;
    while index < chars.len() {
        let character = chars[index];
        if !character.is_ascii_digit() {
            normalized.push(character);
            index += 1;
            continue;
        }
        let run_start = index;
        while index < chars.len() && chars[index].is_ascii_digit() {
            index += 1;
        }
        if is_counting_context(&chars, run_start, index) {
            normalized.push('\u{0}');
        } else {
            normalized.extend(&chars[run_start..index]);
        }
    }
    normalized
}

/// Do the digits at `chars[start..end]` sit in a counting context? Scan the
/// two tokens nearest the run: each matches when it *ends* with a counting
/// keyword, so both `count=27` and the compound `actionCount=27` qualify
/// (recheck N2 — a whole-token comparison would silently exclude compound
/// words). CJK forms (`实际 28`, `共17条`) likewise match by suffix. Error
/// codes, HTTP statuses and path versions keep their digits: `ERROR A4`,
/// `HTTP 500`, `v3` carry no counting keyword.
fn is_counting_context(chars: &[char], _start: usize, end: usize) -> bool {
    let is_separator = |c: char| {
        matches!(
            c,
            ' ' | '=' | ':' | ',' | '，' | '、' | '(' | '（' | ')' | '）' | '[' | '【' | '-'
        )
    };
    let counting = |token: &[char]| -> bool {
        let keyword: String = token.iter().collect();
        // Suffix match on purpose: compound words like `actionCount` must
        // trigger the collapse just like a bare `count` (recheck N2).
        keyword.ends_with("count")
            || keyword.ends_with("actual")
            || keyword.ends_with("got")
            || keyword.ends_with("expected")
            || keyword.ends_with("实际")
            || keyword.ends_with("共")
            || keyword.ends_with("行")
    };
    // Split everything before the run into tokens, then check the two tokens
    // nearest the run: `…的 actionCount=27，实际 28` reads `实际` for the
    // second number and `actioncount` for the first. `tokens` is oldest-first.
    let mut tokens: Vec<&[char]> = Vec::new();
    let mut index = 0usize;
    while index < end {
        if is_separator(chars[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < end && !is_separator(chars[index]) {
            index += 1;
        }
        tokens.push(&chars[start..index]);
    }
    tokens.iter().rev().take(2).any(|token| counting(token))
}

/// Split a trailing occurrence counter off a failure entry. The counter is
/// always appended by [`bump_failure_count`] as ` (repeated xN)`; the phrase
/// is unique enough that literal data ending in it is not a realistic shape,
/// while a bare `[xN]` suffix collides with ordinary data (review M4's
/// `ERROR: crc mismatch [x86]`). Returns `(text, count)`; `count` is 0 for an
/// uncounted entry.
fn strip_failure_counter(entry: &str) -> (&str, u32) {
    const MARKER: &str = " (repeated x";
    if let Some(open) = entry.rfind(MARKER) {
        let inside = &entry[open + MARKER.len()..];
        if let Some(close) = inside.find(')') {
            if close == inside.len() - 1 {
                if let Ok(count) = inside[..close].parse::<u32>() {
                    if count >= 2 {
                        return (&entry[..open], count);
                    }
                }
            }
        }
    }
    (entry, 0)
}

/// Bump or add the trailing occurrence counter on a collapsed failure entry.
/// A bare entry already represents one occurrence, so the first merge shows
/// `(repeated x2)`; an existing counter increments.
fn bump_failure_count(entry: &mut String) {
    let (text, count) = strip_failure_counter(entry);
    *entry = format!("{text} (repeated x{})", count.max(1) + 1);
}

fn tool_call_evidence(call: &ToolCallRef) -> String {
    const KEYS: [&str; 7] = ["path", "file", "command", "query", "url", "pattern", "cwd"];
    let mut args = Vec::new();
    for key in KEYS {
        let Some(value) = call.arguments.get(key) else {
            continue;
        };
        let text = match value {
            serde_json::Value::String(value) => value.clone(),
            serde_json::Value::Bool(value) => value.to_string(),
            serde_json::Value::Number(value) => value.to_string(),
            _ => continue,
        };
        args.push(format!("{key}={}", truncate_middle(&text, 80)));
        if args.len() == 4 {
            break;
        }
    }
    if args.is_empty() {
        call.name.clone()
    } else {
        format!("{}({})", call.name, args.join(", "))
    }
}

fn tool_result_evidence(message: &ModelMessage) -> Option<String> {
    let name = truncate_middle(message.name.as_deref().unwrap_or("tool"), 80);
    let summary = message
        .content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    if is_failure_line(summary) {
        return None;
    }
    let budget = MAX_TOOL_EVIDENCE_CHARS
        .saturating_sub(name.chars().count() + 9)
        .max(1);
    Some(format!(
        "{name} result: {}",
        truncate_middle(summary, budget)
    ))
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars {
        return value.to_string();
    }
    let marker = "[... truncated ...]";
    if max_chars <= marker.chars().count() {
        return marker.chars().take(max_chars).collect();
    }
    let head = max_chars / 4;
    let tail = max_chars - head - marker.chars().count();
    let mut out: String = chars[..head].iter().collect();
    out.push_str(marker);
    out.extend(chars[chars.len() - tail..].iter());
    out
}

fn push_bounded(vec: &mut Vec<String>, value: String, max_chars: usize) {
    let value = truncate_middle(&value, max_chars);
    if !vec.contains(&value) {
        vec.push(value);
    }
}

/// The todo title behind one plan line, for last-wins folding: a `Goal:` line
/// collapses every objective to one slot (the latest objective wins), and a
/// checkbox line keeps its title with the status marker stripped via exact
/// prefixes — char-set stripping would eat a title's leading `x`
/// (`- [ ] xtask migration` colliding with `- [ ] task migration`, review
/// M3), and `line[2..]` byte indexing would panic on a multi-byte first char.
fn todo_title(line: &str) -> String {
    if let Some(rest) = line.strip_prefix("Goal:") {
        return format!("goal:{}", rest.trim_start().to_lowercase());
    }
    let body = line
        .strip_prefix("- [ ] ")
        .or_else(|| line.strip_prefix("- [~] "))
        .or_else(|| line.strip_prefix("- [x] "))
        .unwrap_or(line);
    body.trim().to_string()
}

/// A durable-plan todo row: `- [ ]` / `- [~]` / `- [x]` as rendered by the
/// shared `todo_marker` vocabulary (`ark_server::workbench`, the
/// `todo_write` tool result and the turn-seed plan message).
fn is_plan_checkbox_line(line: &str) -> bool {
    line.starts_with("- [ ]") || line.starts_with("- [~]") || line.starts_with("- [x]")
}

fn is_structured_line(line: &str) -> bool {
    // A plan-seed todo line (`- [ ] title`, from `plan_seed_message` /
    // `todo_write`) carries the durable plan across a compaction boundary:
    // the seed tail is restated every turn, but the checkpoint must also name
    // it so an in-turn compact does not drop the objective list.
    if is_plan_checkbox_line(line) {
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
/// a word boundary after the keyword, and a code-shape rejection for lines
/// that carry JavaScript/call syntax. The keyword-colon arms reject only a
/// value that itself looks like an identifier or literal — the shape of
/// serialized JS object lines — so a real `failed: connect 127.0.0.1
/// refused` survives (review finding S1; capitalised keys covered, M6).
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
/// example a path with parentheses) must stay. The arrow arm demands a bare
/// match-arm / body shape — rustc diagnostics quote signatures in backticks
/// and must survive (M5).
fn is_code_shaped(line: &str) -> bool {
    let trimmed = line.trim_end();
    if trimmed.ends_with(';') || trimmed.ends_with("),") {
        return true;
    }
    if trimmed.ends_with(',') && trimmed.contains('(') {
        return true;
    }
    if trimmed.contains("instanceof") {
        return true;
    }
    if trimmed.contains(" => ") && !trimmed.contains('`') && !trimmed.contains('"') {
        return true;
    }
    // Serialized-JS key/value lines always terminate in `,` or `;`; a real
    // diagnostic sentence does not. Requiring the terminator is what keeps
    // `failed: connect 127.0.0.1 refused` alive while `failed: null,` dies
    // (review S1). Matching runs on the lowercased line so capitalized keys
    // (`Failure: null,`) are caught too (M6); single-quoted values (`failed:
    // 'x',`) are caught by the value whitelist below (recheck N1).
    if !(trimmed.ends_with(',') || trimmed.ends_with(';')) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    keyword_colon_with_code_value(&lower, "failed:")
        || keyword_colon_with_code_value(&lower, "failure:")
}

/// `keyword: <code value><,|;>` shape: the text after the colon starts like an
/// identifier, literal or quoted string. Only called on terminator-terminated
/// lines, so a diagnostic sentence after the colon never reaches this.
fn keyword_colon_with_code_value(line: &str, keyword: &str) -> bool {
    let Some(rest) = line.strip_prefix(keyword) else {
        return false;
    };
    let value = rest.trim_start().trim_end_matches([',', ';']).trim_end();
    let Some(first) = value.chars().next() else {
        return false;
    };
    first.is_ascii_alphanumeric() || first == '_' || first == '$' || first == '"' || first == '\''
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

fn checkpoint_retained_tail(text: &str) -> Option<(usize, u64)> {
    text.lines().find_map(|line| {
        let value = line
            .strip_prefix("[retained tail already summarized: ")?
            .strip_suffix(']')?;
        let (count, fingerprint) = value.split_once("; fingerprint: ")?;
        Some((
            count.parse().ok()?,
            u64::from_str_radix(fingerprint, 16).ok()?,
        ))
    })
}

pub(crate) fn retained_tail_fingerprint<'a>(
    messages: impl IntoIterator<Item = &'a ModelMessage>,
) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for message in messages {
        let bytes = serde_json::to_vec(message).expect("model messages serialize");
        hash = fingerprint_bytes(hash, &(bytes.len() as u64).to_le_bytes());
        hash = fingerprint_bytes(hash, &bytes);
    }
    hash
}

fn fingerprint_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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
