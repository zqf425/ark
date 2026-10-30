//! Pure projections for the Code workbench side panels.
//!
//! The renderer receives ready-to-render data instead of interpreting tool
//! events. These folds are intentionally read-only: the session event log
//! remains the system of record for plan, files, changes, and terminal output.

use std::collections::HashMap;

use ark_contract::{
    FilePreview, PlanView, RuntimeEvent, TerminalEntriesResponse, TerminalEntry, WorkspaceChange,
    WorkspaceChangesResponse,
};

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
    use ark_contract::TodoEntry;
    use chrono::Utc;

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
                todos: vec![TodoEntry {
                    id: "one".into(),
                    title: "write tests".into(),
                    status: ark_contract::TodoStatus::Pending,
                }],
            },
        ];
        let projected = plan(&events);
        assert_eq!(projected.goal.as_deref(), Some("finish the panel"));
        assert_eq!(projected.todos.len(), 1);
    }
}
