//! Rebuilds the durable model-visible prefix for the next turn.

use super::*;

/// Rebuild the durable conversational prefix for the next model turn.
///
/// Reasoning is deliberately excluded: it is renderer-only and must never be
/// replayed as user-visible assistant text. Completed tool calls are folded
/// into the assistant history as `tool_calls`, each followed by a `ToolRole`
/// result message. The store folds `ToolCallStarted` + `ToolCallFinished` into
/// one `ToolCall` item whose payload carries `call_id`/`tool`/`arguments` and
/// the embedded `output`; terminal items replay the full call+result pair.
/// Non-terminal items are dropped because providers reject requests whose
/// tool calls have no result.
pub(super) fn conversation_history(items: &[ark_contract::ThreadItem]) -> Vec<ModelMessage> {
    let mut history: Vec<ModelMessage> = Vec::new();
    for item in items {
        match item.kind {
            ark_contract::item::ItemKind::UserMessage => {
                if let Some(text) = nonempty(item.text.as_deref()) {
                    history.push(ModelMessage::user(text));
                }
            }
            ark_contract::item::ItemKind::AssistantMessage => {
                if let Some(text) = nonempty(item.text.as_deref()) {
                    history.push(ModelMessage::assistant(text));
                }
            }
            ark_contract::item::ItemKind::Notice => {
                if let Some(text) = item
                    .text
                    .as_deref()
                    .filter(|text| text.starts_with("[context checkpoint]"))
                {
                    history.push(ModelMessage::system(text));
                }
            }
            ark_contract::item::ItemKind::ToolCall => {
                let terminal = matches!(
                    item.status,
                    ark_contract::item::ItemStatus::Completed
                        | ark_contract::item::ItemStatus::Failed
                );
                if !terminal {
                    continue;
                }
                let Some(payload) = tool_payload(item) else {
                    continue;
                };
                attach_tool_call(
                    &mut history,
                    ToolCallRef {
                        call_id: payload.call_id.clone(),
                        name: payload.tool.clone(),
                        arguments: payload.arguments.clone().unwrap_or_default(),
                    },
                );
                let call_id = payload.call_id.clone();
                let tool = payload.tool.clone();
                history.push(ModelMessage {
                    role: ModelRole::Tool,
                    content: truncate_utf8(payload.output.as_deref().unwrap_or("")),
                    tool_call_id: Some(call_id),
                    name: Some(tool),
                    tool_calls: Vec::new(),
                });
            }
            _ => {}
        }
    }
    history
}

fn nonempty(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.trim().is_empty())
}

fn tool_payload(item: &ark_contract::ThreadItem) -> Option<&ark_contract::item::ToolPayload> {
    item.tool.as_ref()
}

/// Fold a replayed call into the last assistant row, or open a new one.
/// Ordering of user/assistant rows is never rearranged.
fn attach_tool_call(history: &mut Vec<ModelMessage>, call: ToolCallRef) {
    match history.last_mut() {
        Some(ModelMessage {
            role: ModelRole::Assistant,
            tool_calls,
            ..
        }) => tool_calls.push(call),
        _ => history.push(ModelMessage::assistant_with_tool_calls("", vec![call])),
    }
}

/// Tool outputs are context, not identity: cap them at 16 KiB on a UTF-8
/// boundary so a mid-codepoint cut cannot produce invalid text.
fn truncate_utf8(text: &str) -> String {
    const MAX: usize = 16 * 1024;
    if text.len() <= MAX {
        return text.to_string();
    }
    let mut end = MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// One-line workspace fact. Kept out of the static prefix on purpose: the cache
/// prefix must stay byte-identical between turns and machines.
pub(super) fn workspace_line(workspace: &std::path::Path) -> String {
    format!("workspace root: {}", workspace.display())
}
