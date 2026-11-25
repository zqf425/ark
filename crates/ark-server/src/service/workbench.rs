//! Pure projections for the Code workbench side panels.
//!
//! The renderer receives ready-to-render data instead of interpreting tool
//! events. These folds are intentionally read-only: the session event log
//! remains the system of record for plan, files, changes, and terminal output.

use std::collections::HashMap;

use ark_common::ArkError;
use ark_contract::{
    FilePreview, PlanView, RuntimeEvent, TerminalEntriesResponse, TerminalEntry, TodoEntry,
    TodoStatus, WorkspaceChange, WorkspaceChangesResponse,
};

/// Maximum todos one plan holds. The same bound guards every write path.
const MAX_TODOS: usize = 200;
/// Maximum Unicode scalar values in one title, shared by HTTP and model writes.
pub const MAX_TODO_TITLE_CHARS: usize = 500;

/// Fold the latest goal and todo snapshots from a thread event stream.
pub fn plan(events: &[RuntimeEvent]) -> PlanView {
    let mut view = PlanView::default();
    for event in events {
        match event {
            RuntimeEvent::GoalUpdated { goal, .. } => view.goal = Some(goal.clone()),
            RuntimeEvent::GoalCleared { .. } => view.goal = None,
            RuntimeEvent::TodosUpdated { todos, .. } => view.todos = todos.clone(),
            RuntimeEvent::TodosCleared { .. } => view.todos.clear(),
            _ => {}
        }
    }
    view
}

/// Validate one replacement todo list.
///
/// The single validation host for every writer — the HTTP handler and the
/// `todo_write` tool — so the durable plan cannot be shaped differently by the
/// path that wrote it.
pub fn validate_todos(todos: &[TodoEntry]) -> Result<(), ArkError> {
    if todos.len() > MAX_TODOS {
        return Err(ArkError::validation(format!(
            "a plan supports at most {MAX_TODOS} todos"
        )));
    }
    let mut ids = std::collections::HashSet::new();
    for todo in todos {
        if todo.id.trim().is_empty()
            || todo.title.trim().is_empty()
            || !ids.insert(todo.id.trim().to_string())
        {
            return Err(ArkError::validation(
                "todo ids and titles must be non-empty and ids unique",
            ));
        }
        let title_chars = todo.title.chars().count();
        if title_chars > MAX_TODO_TITLE_CHARS {
            return Err(ArkError::validation(format!(
                "todo `{}` title is {title_chars} characters; keep titles at most {MAX_TODO_TITLE_CHARS}",
                todo.id
            )));
        }
    }
    Ok(())
}

/// The `[ ]` / `[~]` / `[x]` checkbox vocabulary shared by the per-turn plan
/// seed and the `todo_write` result. One renderer, so the model always reads
/// the same todo encoding wherever it appears.
pub fn todo_marker(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => "[ ]",
        TodoStatus::InProgress => "[~]",
        TodoStatus::Completed => "[x]",
    }
}

/// Project successful `write` and `edit` calls into the latest mutation per path.
pub fn changes(events: &[RuntimeEvent]) -> WorkspaceChangesResponse {
    let mut pending: HashMap<String, PendingChange> = HashMap::new();
    let mut latest: HashMap<String, WorkspaceChange> = HashMap::new();
    for event in events {
        match event {
            RuntimeEvent::ToolCallStarted {
                call_id,
                tool,
                arguments,
                turn_id,
                ..
            } if matches!(tool.as_str(), "write" | "edit") => {
                if let Some(path) = argument_string(arguments, "path") {
                    pending.insert(
                        call_id.clone(),
                        PendingChange {
                            path: path.to_string(),
                            operation: tool.clone(),
                            turn_id: *turn_id,
                        },
                    );
                }
            }
            RuntimeEvent::ToolCallFinished {
                call_id, ok, seq, ..
            } => {
                if let Some(change) = pending.remove(call_id) {
                    if *ok {
                        latest.insert(
                            change.path.clone(),
                            WorkspaceChange {
                                path: change.path,
                                operation: change.operation,
                                turn_id: change.turn_id,
                                seq: *seq,
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    let mut changes: Vec<_> = latest.into_values().collect();
    changes.sort_by_key(|change| std::cmp::Reverse(change.seq));
    WorkspaceChangesResponse { changes }
}

/// Return the latest successful `read` result for exactly `path`.
///
/// The runtime does not read the workspace on behalf of a panel: this prevents
/// the UI from bypassing sandbox policy and makes the preview match what the
/// model actually saw.
pub fn file_preview(events: &[RuntimeEvent], path: &str) -> Option<FilePreview> {
    let mut pending: HashMap<String, String> = HashMap::new();
    let mut latest = None;
    for event in events {
        match event {
            RuntimeEvent::ToolCallStarted {
                call_id,
                tool,
                arguments,
                ..
            } if tool == "read" => {
                if let Some(read_path) = argument_string(arguments, "path") {
                    pending.insert(call_id.clone(), read_path.to_string());
                }
            }
            RuntimeEvent::ToolCallFinished {
                call_id,
                ok,
                output,
                seq,
                ..
            } => {
                if let Some(read_path) = pending.remove(call_id) {
                    if *ok && read_path == path {
                        latest = Some(FilePreview {
                            path: read_path,
                            content: output.clone(),
                            source_seq: *seq,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    latest
}

/// Project completed `bash` calls into a bounded terminal history response.
pub fn terminal(events: &[RuntimeEvent]) -> TerminalEntriesResponse {
    let mut pending: HashMap<String, PendingTerminal> = HashMap::new();
    let mut entries = Vec::new();
    for event in events {
        match event {
            RuntimeEvent::ToolCallStarted {
                call_id,
                tool,
                arguments,
                turn_id,
                ..
            } if tool == "bash" => {
                if let Some(command) = argument_string(arguments, "command") {
                    pending.insert(
                        call_id.clone(),
                        PendingTerminal {
                            command: command.to_string(),
                            turn_id: *turn_id,
                        },
                    );
                }
            }
            RuntimeEvent::ToolCallFinished {
                call_id,
                ok,
                output,
                duration_ms,
                seq,
                ..
            } => {
                if let Some(command) = pending.remove(call_id) {
                    entries.push(TerminalEntry {
                        call_id: call_id.clone(),
                        turn_id: command.turn_id,
                        command: command.command,
                        output: output.clone(),
                        ok: *ok,
                        duration_ms: *duration_ms,
                        seq: *seq,
                    });
                }
            }
            _ => {}
        }
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.seq));
    TerminalEntriesResponse { entries }
}

fn argument_string<'a>(arguments: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    arguments
        .get(name)?
        .as_str()
        .filter(|value| !value.trim().is_empty())
}

struct PendingChange {
    path: String,
    operation: String,
    turn_id: ark_common::TurnId,
}

struct PendingTerminal {
    command: String,
    turn_id: ark_common::TurnId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::{ThreadId, TurnId};
    use chrono::Utc;

    fn todo(id: &str, title: &str, status: TodoStatus) -> TodoEntry {
        TodoEntry {
            id: id.into(),
            title: title.into(),
            status,
        }
    }

    #[test]
    fn todos_need_unique_non_empty_ids_and_titles() {
        assert!(validate_todos(&[]).is_ok());
        let duplicate = vec![
            todo("same", "first", TodoStatus::Pending),
            todo("same", "second", TodoStatus::Pending),
        ];
        assert!(validate_todos(&duplicate).is_err());
        assert!(validate_todos(&[todo("a", " ", TodoStatus::Pending)]).is_err());
        assert!(validate_todos(&[todo(" ", "t", TodoStatus::Pending)]).is_err());
    }

    #[test]
    fn todo_validation_bounds_the_list() {
        let many: Vec<TodoEntry> = (0..=200)
            .map(|index| todo(&format!("t{index}"), "row", TodoStatus::Pending))
            .collect();
        assert!(validate_todos(&many).is_err());
        let exact: Vec<TodoEntry> = (0..200)
            .map(|index| todo(&format!("t{index}"), "row", TodoStatus::Pending))
            .collect();
        assert!(validate_todos(&exact).is_ok());
    }

    #[test]
    fn todo_validation_rejects_titles_over_500_characters() {
        assert!(validate_todos(&[todo("a", &"x".repeat(501), TodoStatus::Pending)]).is_err());
        assert!(validate_todos(&[todo("a", &"x".repeat(500), TodoStatus::Pending)]).is_ok());
    }

    #[test]
    fn todo_markers_cover_every_status() {
        assert_eq!(todo_marker(TodoStatus::Pending), "[ ]");
        assert_eq!(todo_marker(TodoStatus::InProgress), "[~]");
        assert_eq!(todo_marker(TodoStatus::Completed), "[x]");
    }

    #[test]
    fn changes_only_include_successful_writes_at_the_latest_path_version() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let now = Utc::now();
        let events = vec![
            RuntimeEvent::ToolCallStarted {
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                timestamp: now,
                call_id: "one".into(),
                tool: "write".into(),
                arguments: serde_json::json!({ "path": "src/main.rs" }),
                item_id: None,
            },
            RuntimeEvent::ToolCallFinished {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                timestamp: now,
                call_id: "one".into(),
                tool: "write".into(),
                ok: true,
                duration_ms: 1,
                output: "written".into(),
                detail: None,
                item_id: None,
            },
        ];
        let projected = changes(&events);
        assert_eq!(projected.changes.len(), 1);
        assert_eq!(projected.changes[0].path, "src/main.rs");
        assert_eq!(projected.changes[0].seq, 2);
    }

    #[test]
    fn plan_uses_the_last_durable_snapshots() {
        let thread = ThreadId::new();
        let now = Utc::now();
        let events = vec![
            RuntimeEvent::GoalUpdated {
                seq: 1,
                thread_id: thread,
                timestamp: now,
                goal: "finish the panel".into(),
            },
            RuntimeEvent::TodosUpdated {
                seq: 2,
                thread_id: thread,
                timestamp: now,
                todos: vec![todo("one", "write tests", TodoStatus::Pending)],
            },
        ];
        let projected = plan(&events);
        assert_eq!(projected.goal.as_deref(), Some("finish the panel"));
        assert_eq!(projected.todos.len(), 1);
    }
}
