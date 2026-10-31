//! History hygiene: repair an illegal `tool_call` / `tool_result` history before
//! it reaches a provider (design §7.1 `context/`).
//!
//! # Current contract
//!
//! `ark_ports::ModelMessage` carries `tool_calls`, so an assistant row names the
//! calls it issued and a tool row answers one specific call id. Providers accept
//! exactly that shape, including an assistant row whose `content` is empty
//! because the model answered with calls only (deepseek-class models send
//! reasoning plus calls and no text). This module therefore repairs *pairing*
//! — orphan, unidentified and repeated results, empty rows — instead of
//! compensating for a missing contract field.

use std::collections::BTreeSet;

use ark_ports::{ModelMessage, ModelRole};

/// What the repair changed. Always returned so the caller can emit an event
/// instead of silently mutating history.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HygieneReport {
    /// Tool results dropped because nothing answered them.
    pub dropped_orphan_results: u32,
    /// Tool results dropped because the call id repeated inside one run.
    pub dropped_duplicate_results: u32,
    /// Tool results dropped because `tool_call_id` was missing.
    pub dropped_unidentified_results: u32,
    /// Empty user rows, and assistant rows carrying neither text nor
    /// `tool_calls`, removed.
    pub dropped_empty_messages: u32,
    /// Consecutive duplicate system messages collapsed.
    pub collapsed_system_messages: u32,
}

impl HygieneReport {
    pub fn changed(&self) -> bool {
        *self != HygieneReport::default()
    }

    /// Total number of messages removed.
    pub fn removed(&self) -> u32 {
        self.dropped_orphan_results
            + self.dropped_duplicate_results
            + self.dropped_unidentified_results
            + self.dropped_empty_messages
            + self.collapsed_system_messages
    }
}

/// Stateless repair pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct HistoryHygiene;

impl HistoryHygiene {
    pub fn new() -> Self {
        Self
    }

    /// Repair `messages` in place and report what changed.
    ///
    /// Rules, in order:
    /// 1. A `Tool` message without `tool_call_id` cannot be matched by any
    ///    provider, so it is dropped.
    /// 2. A `Tool` message before the first assistant message has no call to
    ///    answer, so it is dropped.
    /// 3. A repeated `tool_call_id` inside one assistant run is dropped: one
    ///    call answers once. The run spans consecutive assistant rows, so a call
    ///    id repeated by a later row is still caught.
    /// 4. An empty `User` message, and an `Assistant` message with neither text
    ///    nor `tool_calls`, carries nothing and is dropped. An assistant row
    ///    with empty `content` but non-empty `tool_calls` is the standard
    ///    chat-completions shape and is kept — dropping it would orphan every
    ///    result that answers it.
    /// 5. Consecutive identical `System` messages are collapsed.
    ///
    /// The starting-point rule for step 2 and step 3 is "no assistant message
    /// has been seen yet since the last user message", which mirrors what the
    /// chat formats accept.
    pub fn repair(&self, messages: &mut Vec<ModelMessage>) -> HygieneReport {
        let mut report = HygieneReport::default();
        let mut kept: Vec<ModelMessage> = Vec::with_capacity(messages.len());
        let mut since_last_user = false;
        // Call ids already answered since the last user message. It survives
        // assistant rows on purpose: a duplicate is judged over the whole run,
        // not only over the rows following one assistant message.
        let mut answered_call_ids: BTreeSet<String> = BTreeSet::new();

        for message in messages.drain(..) {
            match message.role {
                ModelRole::System => {
                    let duplicate = kept.last().is_some_and(|previous| {
                        previous.role == ModelRole::System && previous.content == message.content
                    });
                    if duplicate {
                        report.collapsed_system_messages += 1;
                        continue;
                    }
                    kept.push(message);
                }
                ModelRole::User => {
                    if message.content.trim().is_empty() {
                        report.dropped_empty_messages += 1;
                        continue;
                    }
                    // A new user message closes any open assistant run.
                    since_last_user = false;
                    answered_call_ids.clear();
                    kept.push(message);
                }
                ModelRole::Assistant => {
                    if message.content.trim().is_empty() && message.tool_calls.is_empty() {
                        report.dropped_empty_messages += 1;
                        continue;
                    }
                    since_last_user = true;
                    // Declared call ids are not pre-claimed here: the first
                    // result answering a call claims its id, and a repeat of
                    // that id — from this row or a later one — is then the
                    // duplicate the next rule drops.
                    kept.push(message);
                }
                ModelRole::Tool => {
                    let Some(call_id) = message.tool_call_id.clone() else {
                        report.dropped_unidentified_results += 1;
                        continue;
                    };
                    if !since_last_user {
                        report.dropped_orphan_results += 1;
                        continue;
                    }
                    if !answered_call_ids.insert(call_id) {
                        report.dropped_duplicate_results += 1;
                        continue;
                    }
                    kept.push(message);
                }
            }
        }

        *messages = kept;
        report
    }

    /// True when the history would currently be rejected by a chat provider:
    /// a tool result exists with no assistant message in front of it.
    pub fn is_legal(&self, messages: &[ModelMessage]) -> bool {
        let mut since_last_user = false;
        for message in messages {
            match message.role {
                ModelRole::Assistant => since_last_user = true,
                ModelRole::User => since_last_user = false,
                ModelRole::Tool => {
                    if !since_last_user || message.tool_call_id.is_none() {
                        return false;
                    }
                }
                ModelRole::System => {}
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_result(call_id: Option<&str>, content: &str) -> ModelMessage {
        ModelMessage {
            role: ModelRole::Tool,
            content: content.to_string(),
            tool_call_id: call_id.map(str::to_string),
            name: None,
            tool_calls: Vec::new(),
        }
    }

    fn assistant_call(content: &str, call_ids: &[&str]) -> ModelMessage {
        let calls = call_ids
            .iter()
            .map(|call_id| ark_ports::ToolCallRef {
                call_id: (*call_id).to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"path": "a.txt"}),
            })
            .collect();
        ModelMessage::assistant_with_tool_calls(content, calls)
    }

    #[test]
    fn legal_history_is_untouched() {
        let mut messages = vec![
            ModelMessage::system("sys"),
            ModelMessage::user("hi"),
            ModelMessage::assistant("calling read"),
            tool_result(Some("call_1"), "file body"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert!(!report.changed());
        assert_eq!(messages.len(), 4);
        assert!(HistoryHygiene::new().is_legal(&messages));
    }

    #[test]
    fn orphan_result_before_first_assistant_is_dropped() {
        let mut messages = vec![
            ModelMessage::system("sys"),
            ModelMessage::user("hi"),
            tool_result(Some("call_1"), "stray"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert_eq!(report.dropped_orphan_results, 1);
        assert_eq!(messages.len(), 2);
        assert!(HistoryHygiene::new().is_legal(&messages));
    }

    #[test]
    fn result_without_call_id_is_dropped() {
        let mut messages = vec![
            ModelMessage::user("hi"),
            ModelMessage::assistant("calling"),
            tool_result(None, "no id"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert_eq!(report.dropped_unidentified_results, 1);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn duplicate_call_id_is_dropped() {
        let mut messages = vec![
            ModelMessage::user("hi"),
            ModelMessage::assistant("calling twice"),
            tool_result(Some("call_1"), "first"),
            tool_result(Some("call_1"), "duplicate"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert_eq!(report.dropped_duplicate_results, 1);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2].content, "first");
    }

    #[test]
    fn empty_assistant_with_tool_calls_keeps_its_results() {
        // The shape deepseek-class models replay: no text, calls only. Dropping
        // this row orphans every result that answers it.
        let mut messages = vec![
            ModelMessage::user("hi"),
            assistant_call("", &["call_1", "call_2"]),
            tool_result(Some("call_1"), "first body"),
            tool_result(Some("call_2"), "second body"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert!(
            !report.changed(),
            "a call-only assistant row is legal history: {report:?}"
        );
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[1].tool_calls.len(), 2);
        assert_eq!(messages[2].content, "first body");
        assert_eq!(messages[3].content, "second body");
        assert!(HistoryHygiene::new().is_legal(&messages));
    }

    #[test]
    fn whitespace_only_assistant_with_tool_calls_is_kept() {
        let mut messages = vec![
            ModelMessage::user("hi"),
            assistant_call("   ", &["call_1"]),
            tool_result(Some("call_1"), "body"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert!(!report.changed(), "report: {report:?}");
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn assistant_without_text_or_calls_is_still_dropped() {
        let mut messages = vec![
            ModelMessage::user("hi"),
            ModelMessage::assistant(""),
            assistant_call("", &[]),
            ModelMessage::user("real"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert_eq!(report.dropped_empty_messages, 2);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].content, "real");
    }

    #[test]
    fn duplicate_call_id_across_empty_assistant_rows_is_dropped() {
        let mut messages = vec![
            ModelMessage::user("hi"),
            assistant_call("", &["call_1"]),
            tool_result(Some("call_1"), "first"),
            assistant_call("", &["call_1"]),
            tool_result(Some("call_1"), "duplicate"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert_eq!(
            report.dropped_duplicate_results, 1,
            "the run spans assistant rows: {report:?}"
        );
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2].content, "first");
    }

    #[test]
    fn new_user_message_closes_the_assistant_run() {
        let mut messages = vec![
            ModelMessage::user("hi"),
            ModelMessage::assistant("calling"),
            tool_result(Some("call_1"), "ok"),
            ModelMessage::user("next"),
            tool_result(Some("call_2"), "orphan"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert_eq!(report.dropped_orphan_results, 1);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages.last().unwrap().role, ModelRole::User);
    }

    #[test]
    fn empty_user_and_assistant_messages_are_dropped() {
        let mut messages = vec![
            ModelMessage::system("sys"),
            ModelMessage::user("   "),
            ModelMessage::assistant(""),
            ModelMessage::user("real"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert_eq!(report.dropped_empty_messages, 2);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].content, "real");
    }

    #[test]
    fn consecutive_duplicate_systems_collapse() {
        let mut messages = vec![
            ModelMessage::system("prefix"),
            ModelMessage::system("prefix"),
            ModelMessage::user("hi"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert_eq!(report.collapsed_system_messages, 1);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn distinct_systems_are_both_kept() {
        let mut messages = vec![
            ModelMessage::system("a"),
            ModelMessage::system("b"),
            ModelMessage::user("hi"),
        ];
        assert!(!HistoryHygiene::new().repair(&mut messages).changed());
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn report_counts_add_up() {
        let report = HygieneReport {
            dropped_orphan_results: 1,
            dropped_duplicate_results: 2,
            dropped_unidentified_results: 3,
            dropped_empty_messages: 4,
            collapsed_system_messages: 5,
        };
        assert_eq!(report.removed(), 15);
        assert!(report.changed());
    }

    #[test]
    fn multiple_sequential_tool_runs_stay_legal() {
        let mut messages = vec![
            ModelMessage::user("hi"),
            ModelMessage::assistant("call a"),
            tool_result(Some("call_a"), "a"),
            ModelMessage::assistant("call b"),
            tool_result(Some("call_b"), "b"),
        ];
        let report = HistoryHygiene::new().repair(&mut messages);
        assert!(
            !report.changed(),
            "a second assistant run resets the open ids"
        );
        assert_eq!(messages.len(), 5);
    }
}
