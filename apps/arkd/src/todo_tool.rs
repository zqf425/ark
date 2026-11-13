//! The model-facing `todo_write` tool belongs in the composition root: it
//! writes through the concrete `SessionStore` without making `ark-tool` depend
//! on `ark-store`.
//!
//! The tool is the model's only path into the durable plan panel. Because the
//! panel renders from `TodosUpdated` events and the per-turn seed restates the
//! plan from the same event log, a todo the model writes here survives context
//! compaction by construction: compaction folds history, never the seed tail.

use std::sync::Arc;

use ark_common::Result;
use ark_contract::{RuntimeEvent, TodoEntry};
use ark_ports::{SessionStore, Tool, ToolArgs, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

/// Bounded titles so one verbose model call cannot bloat the panel or the
/// per-turn seed. Titles beyond the cap are rejected with a message the model
/// can act on; truncating silently would let the plan claim something the
/// model never wrote.
const MAX_TODO_TITLE_CHARS: usize = 500;

static TODO_WRITE_SCHEMA: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
    json!({
        "type": "object",
        "description": "Replace the durable thread todo list shown in the session plan panel. Send the complete list every time: entries the model wrote earlier must be included unchanged or they are dropped. The list is restated to the model at every turn, including after context compaction, so non-completed entries keep the work on track.",
        "properties": {
            "todos": {
                "type": "array",
                "description": "The complete ordered todo list, oldest first.",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Stable unique id for the entry; reuse the id of an entry you are updating so the panel edits in place." },
                        "title": { "type": "string", "description": "Short imperative description of the remaining work." },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed"],
                            "description": "pending = not started, in_progress = actively worked on, completed = done and verified."
                        }
                    },
                    "required": ["id", "title", "status"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["todos"],
        "additionalProperties": false
    })
});

#[derive(Debug, Deserialize)]
struct TodoWriteArgs {
    #[serde(default)]
    todos: Vec<TodoEntry>,
}

/// Replaces the thread's durable todo list through an authoritative
/// `TodosUpdated` event.
pub struct TodoWriteTool {
    store: Arc<dyn SessionStore>,
}

impl TodoWriteTool {
    pub fn new(store: Arc<dyn SessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &'static str {
        "todo_write"
    }

    fn schema(&self) -> &'static Value {
        &TODO_WRITE_SCHEMA
    }

    async fn execute(&self, ctx: &ToolContext, args: ToolArgs) -> Result<ToolOutput> {
        let parsed: TodoWriteArgs = serde_json::from_value(args.0.clone()).map_err(|err| {
            ark_common::ArkError::validation(format!("invalid arguments for `todo_write`: {err}"))
        })?;
        // Shared with the HTTP route: one validator, so the durable plan
        // cannot be shaped differently by the path that wrote it. Expected
        // failures answer as `ToolOutput::err` so the model can read the
        // reason and correct its next call, like every other builtin.
        if let Err(err) = ark_server::service::workbench::validate_todos(&parsed.todos) {
            return Ok(ToolOutput::err(err.message));
        }
        for todo in &parsed.todos {
            let title_chars = todo.title.chars().count();
            if title_chars > MAX_TODO_TITLE_CHARS {
                return Ok(ToolOutput::err(format!(
                    "todo `{}` title is {title_chars} characters; keep titles at most {MAX_TODO_TITLE_CHARS}",
                    todo.id
                )));
            }
        }
        let thread = ctx.thread_id;
        // The thread must exist before the event lands; an unknown id is a
        // model-visible failure, not a silently orphaned event.
        if let Err(err) = self.store.get_thread(thread).await {
            return Ok(ToolOutput::err(format!(
                "todo_write failed: the current thread no longer exists ({})",
                err.message
            )));
        }
        self.store
            .append_event(
                thread,
                RuntimeEvent::TodosUpdated {
                    seq: 0,
                    thread_id: thread,
                    timestamp: chrono::Utc::now(),
                    todos: parsed.todos,
                },
            )
            .await?;
        let events = self.store.replay_events(thread, 0).await?;
        Ok(ToolOutput::ok(render_plan(
            &ark_server::service::workbench::plan(&events),
        )))
    }
}

/// The post-write plan the tool answers with, in the shared checkbox
/// vocabulary. Echoing the durable state (not the request) closes the loop on
/// last-writer-wins and lets the model verify its own write.
pub fn render_plan(plan: &ark_contract::PlanView) -> String {
    let mut lines = vec![format!(
        "todo list saved: {} entr{}",
        plan.todos.len(),
        if plan.todos.len() == 1 { "y" } else { "ies" }
    )];
    if let Some(goal) = plan.goal.as_deref() {
        lines.push(format!("Goal: {}", goal.trim()));
    }
    for todo in &plan.todos {
        let mark = ark_server::service::workbench::todo_marker(todo.status);
        lines.push(format!("- {mark} {}", todo.title.trim()));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::ThreadId;
    use ark_contract::TodoStatus;
    use ark_store::{FileSessionStore, StoreConfig};

    fn store(data_dir: &std::path::Path) -> Arc<dyn SessionStore> {
        Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("session store"))
    }

    fn context(thread: ThreadId) -> ToolContext {
        ToolContext {
            thread_id: thread,
            turn_id: ark_common::TurnId::new(),
            workspace: std::env::temp_dir(),
            sandbox: ark_ports::SandboxDecision::WorkspaceWrite,
            env: std::collections::BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn todo_write_appends_the_durable_event_and_echoes_the_plan() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        let thread = store.create_thread("t").await.unwrap().thread_id;
        let tool = TodoWriteTool::new(Arc::clone(&store));

        let output = tool
            .execute(
                &context(thread),
                ToolArgs(serde_json::json!({
                    "todos": [
                        { "id": "1", "title": "read files", "status": "completed" },
                        { "id": "2", "title": "fix wire", "status": "in_progress" },
                        { "id": "3", "title": "run tests", "status": "pending" }
                    ]
                })),
            )
            .await
            .unwrap();

        assert!(output.ok, "{}", output.content);
        assert!(
            output.content.contains("- [x] read files"),
            "{}",
            output.content
        );
        assert!(output.content.contains("- [~] fix wire"));
        assert!(output.content.contains("- [ ] run tests"));

        let events = store.replay_events(thread, 0).await.unwrap();
        let updated = events
            .iter()
            .find_map(|event| match event {
                RuntimeEvent::TodosUpdated { todos, .. } => Some(todos.clone()),
                _ => None,
            })
            .expect("TodosUpdated event appended");
        assert_eq!(updated.len(), 3);
        assert_eq!(updated[0].status, TodoStatus::Completed);
    }

    #[tokio::test]
    async fn todo_write_rejects_duplicate_ids_and_overlong_titles() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store(dir.path());
        let thread = store.create_thread("t").await.unwrap().thread_id;
        let tool = TodoWriteTool::new(store);

        let duplicate = tool
            .execute(
                &context(thread),
                ToolArgs(serde_json::json!({
                    "todos": [
                        { "id": "1", "title": "a", "status": "pending" },
                        { "id": "1", "title": "b", "status": "pending" }
                    ]
                })),
            )
            .await
            .unwrap();
        assert!(!duplicate.ok);
        assert!(
            duplicate.content.contains("unique"),
            "{}",
            duplicate.content
        );

        let long_title = "x".repeat(MAX_TODO_TITLE_CHARS + 1);
        let overlong = tool
            .execute(
                &context(thread),
                ToolArgs(serde_json::json!({
                    "todos": [{ "id": "1", "title": long_title, "status": "pending" }]
                })),
            )
            .await
            .unwrap();
        assert!(!overlong.ok);
        assert!(
            overlong.content.contains("characters"),
            "{}",
            overlong.content
        );
    }

    #[tokio::test]
    async fn todo_write_reports_an_unknown_thread_instead_of_orphaning_the_event() {
        let dir = tempfile::tempdir().expect("temp dir");
        let tool = TodoWriteTool::new(store(dir.path()));

        let output = tool
            .execute(
                &context(ThreadId::new()),
                ToolArgs(serde_json::json!({
                    "todos": [{ "id": "1", "title": "a", "status": "pending" }]
                })),
            )
            .await
            .unwrap();
        assert!(!output.ok, "{}", output.content);
    }

    #[test]
    fn render_plan_reports_an_empty_list_explicitly() {
        let rendered = render_plan(&ark_contract::PlanView::default());
        assert_eq!(rendered, "todo list saved: 0 entries");
    }
}
