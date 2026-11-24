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
///
/// The newest context checkpoint is a durable compaction boundary: every row
/// before it was folded into that note by an earlier turn. Replaying those rows
/// would silently undo compaction on the very next turn. The original items
/// remain on disk (rewind / export can still reach them); only the
/// model-visible prefix is cut here. Identity comes from the structural
/// marker, never from the note's text.
pub(super) fn conversation_history(items: &[ark_contract::ThreadItem]) -> Vec<ModelMessage> {
    ark_agent::model_history(items)
}

/// Restate the durable thread goal and todos near the end of the seed.
///
/// A long history pushes early goals out of the model's effective attention
/// window (lost-in-the-middle). Re-emitting the current plan each turn keeps
/// the objective and progress in the recent tail without duplicating the whole
/// transcript. Returns `None` when the thread has no plan.
///
/// The todo lines use the shared `[ ]` / `[~]` / `[x]` vocabulary from
/// `ark_server` — the same renderer the `todo_write` tool
/// answers with — so the model reads one encoding wherever todos appear.
pub(super) fn plan_seed_message(plan: &ark_contract::PlanView) -> Option<ModelMessage> {
    if plan.goal.is_none() && plan.todos.is_empty() {
        return None;
    }
    let mut lines = vec!["[thread plan - current objective and progress]".to_string()];
    if let Some(goal) = plan.goal.as_deref() {
        lines.push(format!("Goal: {}", goal.trim()));
    }
    for todo in &plan.todos {
        let mark = ark_server::todo_marker(todo.status);
        lines.push(format!("- {mark} {}", todo.title.trim()));
    }
    Some(ModelMessage::system(lines.join("\n")))
}

/// One-line workspace fact. Kept out of the static prefix on purpose: the cache
/// prefix must stay byte-identical between turns and machines.
pub(super) fn workspace_line(workspace: &std::path::Path) -> String {
    format!("workspace root: {}", workspace.display())
}
