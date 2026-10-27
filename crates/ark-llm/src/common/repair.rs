//! Repair helpers (design §7.2 `common/repair.rs`).
//!
//! Two jobs, both about malformed data on the wire rather than bad code:
//!
//! - a tool call whose argument JSON is truncated or wrapped must still become a
//!   usable `serde_json::Value`, never a panic or a silently dropped call;
//! - a message list must satisfy the pairing rules the providers enforce before
//!   it is serialised.

use ark_ports::{ModelMessage, ModelRole};

/// Parse tool arguments defensively.
///
/// Truncated JSON (a stream that ended early) yields an empty object so the call
/// can still be dispatched and the tool can report a real argument error. A
/// double-encoded string is unwrapped once, because some providers send the
/// argument object as a JSON string.
pub fn parse_tool_arguments(raw: &str) -> serde_json::Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return serde_json::Value::Object(Default::default());
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(serde_json::Value::String(inner)) => {
            // Unwrap one level of accidental double encoding.
            serde_json::from_str::<serde_json::Value>(&inner)
                .unwrap_or(serde_json::Value::String(inner))
        }
        Ok(value) => value,
        Err(_) => best_effort_object(trimmed),
    }
}

/// Salvage `"key": value` pairs from truncated JSON.
///
/// Only used when a full parse already failed, so a correct payload never takes
/// this path.
fn best_effort_object(raw: &str) -> serde_json::Value {
    let start = raw.find('{');
    let end = raw.rfind('}');
    let candidate = match (start, end) {
        (Some(start), Some(end)) if end > start => &raw[start..=end],
        (Some(start), None) => &raw[start..],
        _ => return serde_json::Value::Object(Default::default()),
    };
    // Close any unbalanced braces/brackets left by a truncated stream.
    let mut repaired = candidate.to_string();
    let opens = repaired.matches('{').count();
    let closes = repaired.matches('}').count();
    for _ in 0..opens.saturating_sub(closes) {
        repaired.push('}');
    }
    match serde_json::from_str::<serde_json::Value>(&repaired) {
        Ok(value) => value,
        Err(_) => serde_json::Value::Object(Default::default()),
    }
}

/// Ensure the message list is serialisable by every provider.
///
/// Rules: a tool result must carry `tool_call_id`; a leading tool result with no
/// assistant message before it is dropped; consecutive duplicate system messages
/// are collapsed. This is the adapter's last line of defence — the agent already
/// repairs history, but an adapter must not trust its caller.
pub fn repair_messages(messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    let mut repaired: Vec<ModelMessage> = Vec::with_capacity(messages.len());
    let mut saw_assistant = false;
    for message in messages {
        match message.role {
            ModelRole::System => {
                let duplicate = repaired.last().is_some_and(|previous: &ModelMessage| {
                    previous.role == ModelRole::System && previous.content == message.content
                });
                if !duplicate {
                    repaired.push(message);
                }
            }
            ModelRole::Assistant => {
                saw_assistant = true;
                repaired.push(message);
            }
            ModelRole::User => {
                saw_assistant = false;
                repaired.push(message);
            }
            ModelRole::Tool => {
                if message.tool_call_id.is_some() && saw_assistant {
                    repaired.push(message);
                }
            }
        }
    }
    repaired
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_result(call_id: Option<&str>) -> ModelMessage {
        ModelMessage {
            role: ModelRole::Tool,
            content: "result".into(),
            tool_call_id: call_id.map(str::to_string),
            name: None,
        }
    }

    #[test]
    fn valid_arguments_roundtrip() {
        assert_eq!(
            parse_tool_arguments("{\"path\":\"a\"}"),
            json!({"path": "a"})
        );
    }

    #[test]
    fn empty_arguments_become_an_object() {
        assert_eq!(parse_tool_arguments("   "), json!({}));
    }

    #[test]
    fn truncated_json_is_salvaged() {
        // A stream cut after the value: the tool can still see `path`.
        let value = parse_tool_arguments("{\"path\":\"a.txt\"");
        assert_eq!(value["path"], "a.txt");
    }

    #[test]
    fn double_encoded_arguments_are_unwrapped() {
        let raw = serde_json::to_string(&json!({"path": "a"})).unwrap();
        let encoded = serde_json::to_string(&raw).unwrap();
        assert_eq!(parse_tool_arguments(&encoded), json!({"path": "a"}));
    }

    #[test]
    fn garbage_becomes_an_empty_object() {
        assert_eq!(parse_tool_arguments("}{ not json"), json!({}));
        assert_eq!(parse_tool_arguments("nonsense"), json!({}));
    }

    #[test]
    fn non_object_json_is_preserved() {
        assert_eq!(parse_tool_arguments("[1,2]"), json!([1, 2]));
        assert_eq!(parse_tool_arguments("42"), json!(42));
    }

    #[test]
    fn repair_pairs_results_with_an_assistant() {
        let messages = vec![
            ModelMessage::system("sys"),
            ModelMessage::user("hi"),
            ModelMessage::assistant("calling"),
            tool_result(Some("c1")),
        ];
        assert_eq!(repair_messages(messages.clone()).len(), 4);
    }

    #[test]
    fn repair_drops_orphan_and_unidentified_results() {
        let messages = vec![
            ModelMessage::user("hi"),
            tool_result(Some("c1")),
            ModelMessage::assistant("calling"),
            tool_result(None),
            tool_result(Some("c2")),
        ];
        let repaired = repair_messages(messages);
        assert_eq!(repaired.len(), 3);
        assert_eq!(repaired[2].tool_call_id.as_deref(), Some("c2"));
    }

    #[test]
    fn repair_collapses_duplicate_systems() {
        let messages = vec![
            ModelMessage::system("prefix"),
            ModelMessage::system("prefix"),
            ModelMessage::user("hi"),
        ];
        assert_eq!(repair_messages(messages).len(), 2);
    }

    #[test]
    fn repair_clears_the_assistant_run_at_a_user_message() {
        let messages = vec![
            ModelMessage::user("one"),
            ModelMessage::assistant("a"),
            tool_result(Some("c1")),
            ModelMessage::user("two"),
            tool_result(Some("c2")),
        ];
        let repaired = repair_messages(messages);
        assert_eq!(repaired.len(), 4);
        assert_eq!(repaired[3].role, ModelRole::User);
    }
}
