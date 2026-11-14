//! The MCP tool catalog: one entry per REST capability an external agent may
//! call (requirements/mcp-server-surface.md).
//!
//! This table is the only tool catalog; `ark-contract` remains the single wire
//! truth. Schemas therefore stay lean: path parameters and the few fields a
//! caller must get right are written out, while request bodies modelled by a
//! contract DTO are referenced by type name instead of being re-listed field by
//! field: re-listing them here would be a second copy that drifts. The
//! router-consistency test in `crates/ark-server/tests/mcp_info_surface.rs`
//! keeps this table honest against the route table.

use serde_json::{json, Value};

use super::mcp_info::McpToolDoc;

fn tool(
    name: &'static str,
    description: &'static str,
    method: &'static str,
    path: &'static str,
    properties: Value,
    required: &[&str],
) -> McpToolDoc {
    let mut schema = json!({ "type": "object", "properties": properties });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    McpToolDoc::new(name, description, method, path, schema)
}

/// Path parameter: thread id.
fn thread_id() -> Value {
    json!({ "type": "string", "description": "Path parameter: thread id." })
}

/// Schema for a body whose shape is owned by a `ark-contract` DTO: name the
/// type (the generated TypeScript in `web/src/generated` mirrors it) instead of
/// re-listing the fields.
fn dto_body(type_name: &str) -> Value {
    json!({
        "type": "object",
        "description": format!(
            "Body shape: `{type_name}` from `ark-contract` (mirrored in `web/src/generated`)."
        )
    })
}

/// Schema for a body whose type lives in `ark-server` (route-local request
/// shapes that are not part of the generated contract). The description must
/// say so: a caller following the pointer to `web/src/generated` would find
/// nothing.
fn local_body(type_name: &str) -> Value {
    json!({
        "type": "object",
        "description": format!(
            "Body shape: `{type_name}`, defined in `ark-server` route handlers (not a contract DTO)."
        )
    })
}

mod platform;

pub(crate) fn tools() -> Vec<McpToolDoc> {
    let mut tools = Vec::new();
    threads_lifecycle(&mut tools);
    threads_history(&mut tools);
    threads_insights(&mut tools);
    threads_goal_todos(&mut tools);
    turns(&mut tools);
    gates(&mut tools);
    schedules(&mut tools);
    memory(&mut tools);
    attachments(&mut tools);
    child_runs(&mut tools);
    platform::tools(&mut tools);
    tools
}

fn threads_lifecycle(tools: &mut Vec<McpToolDoc>) {
    tools.extend([

        tool(
            "thread_list",
            "List threads: active first, then most recently updated. Absent `limit` lists everything.",
            "GET",
            "/v1/threads",
            json!({
                "archived": { "type": "boolean", "description": "Filter by archived flag; absent returns both." },
                "q": { "type": "string", "description": "Case-insensitive substring filter on the title." },
                "limit": { "type": "integer", "minimum": 1, "description": "Page size; clamped to 200 when present." },
                "offset": { "type": "integer", "minimum": 0, "description": "Zero-based offset into the deterministic order." }
            }),
            &[],
        ),

        tool(
            "thread_create",
            "Create a thread. Returns 201 with the new thread summary.",
            "POST",
            "/v1/threads",
            json!({
                "title": { "type": "string", "description": "Optional; a placeholder is used when absent." },
                "workspace": { "type": "string", "description": "Optional workspace path for the new thread; validated against the filesystem when present." }
            }),
            &[],
        ),

        tool(
            "thread_get",
            "Read one thread in full: summary, items, child overview and runtime snapshot.",
            "GET",
            "/v1/threads/{id}",
            json!({ "id": thread_id() }),
            &["id"],
        ),

        tool(
            "thread_update",
            "Rename, archive/unarchive, or override per-thread runtime (provider/model/autonomy). Absent fields keep stored values.",
            "PATCH",
            "/v1/threads/{id}",
            json!({
                "id": thread_id(),
                "body": dto_body("UpdateThreadRequest")
            }),
            &["id"],
        ),

        tool(
            "thread_delete",
            "Delete the thread directory and its whole event log. Answers 409 while a turn is running; interrupt first. Use `thread_delete_by_workspace` to sweep one project's settled sessions.",
            "DELETE",
            "/v1/threads/{id}",
            json!({ "id": thread_id() }),
            &["id"],
        ),

        tool(
            "thread_delete_by_workspace",
            "Bulk delete every settled thread whose workspace matches the query parameter; threads with a running turn are skipped and reported with code `conflict`.",
            "DELETE",
            "/v1/threads",
            json!({
                "workspace": { "type": "string", "description": "Required project workspace path; matched against each thread's workspace override." }
            }),
            &[],
        ),

        tool(
            "thread_fork",
            "Fork the thread at the current durable watermark into a new thread. Returns 201 with the fork summary.",
            "POST",
            "/v1/threads/{id}/fork",
            json!({ "id": thread_id() }),
            &["id"],
        ),

    ]);
}

fn threads_history(tools: &mut Vec<McpToolDoc>) {
    tools.extend([

        tool(
            "thread_rewind",
            "Physically rewind: delete the target and everything after it, invalidate old SSE state. Provide `turn_id` or `seq`, never both.",
            "POST",
            "/v1/threads/{id}/rewind",
            json!({
                "id": thread_id(),
                "body": dto_body("RewindRequest (turn_id xor seq)")
            }),
            &["id"],
        ),

        tool(
            "thread_compact",
            "Manually compact the thread context; returns the checkpoint text.",
            "POST",
            "/v1/threads/{id}/compact",
            json!({ "id": thread_id() }),
            &["id"],
        ),

        tool(
            "thread_export",
            "Export a deterministic, portable session JSON: metadata plus the ascending authoritative event log, no credentials or attachment payloads.",
            "GET",
            "/v1/threads/{id}/export",
            json!({ "id": thread_id() }),
            &["id"],
        ),

        tool(
            "thread_review",
            "Start a code-review turn (202 + TurnSummary). The prompt is built from the thread's authoritative workspace-change projection.",
            "POST",
            "/v1/threads/{id}/review",
            json!({
                "id": thread_id(),
                "body": dto_body("ReviewRequest (optional focus)")
            }),
            &["id"],
        ),

        tool(
            "thread_events",
            "Subscribe to the thread event stream (SSE). Read `last_seq` from thread_get first and pass it as `since_seq` to avoid replaying history.",
            "GET",
            "/v1/threads/{id}/events",
            json!({
                "id": thread_id(),
                "since_seq": { "type": "integer", "minimum": 0, "description": "Replay durable events after this seq; defaults to 0." }
            }),
            &["id"],
        ),

        tool(
            "thread_usage",
            "Project the thread's model usage and error attribution from authoritative usage events.",
            "GET",
            "/v1/threads/{id}/usage",
            json!({
                "id": thread_id(),
                "limit": { "type": "integer", "minimum": 1 },
                "offset": { "type": "integer", "minimum": 0 }
            }),
            &["id"],
        ),

    ]);
}

fn threads_insights(tools: &mut Vec<McpToolDoc>) {
    tools.extend([

        tool(
            "thread_skills",
            "List skills visible to this thread (global roots plus thread extras), with activation hints.",
            "GET",
            "/v1/threads/{id}/skills",
            json!({ "id": thread_id() }),
            &["id"],
        ),

        tool(
            "thread_skills_refresh",
            "Rescan the skills directories and refresh the thread view.",
            "POST",
            "/v1/threads/{id}/skills/refresh",
            json!({ "id": thread_id() }),
            &["id"],
        ),

        tool(
            "thread_plan",
            "Read the workbench Plan projection (goal + todos) from authoritative events.",
            "GET",
            "/v1/threads/{id}/plan",
            json!({ "id": thread_id() }),
            &["id"],
        ),

        tool(
            "thread_changes",
            "Read the thread's workspace change projection (from persisted tool output, never the disk directly).",
            "GET",
            "/v1/threads/{id}/changes",
            json!({ "id": thread_id() }),
            &["id"],
        ),

        tool(
            "thread_files",
            "Read the thread's persisted `read` tool file outputs.",
            "GET",
            "/v1/threads/{id}/files",
            json!({
                "id": thread_id(),
                "turn_id": { "type": "string", "description": "Optional: restrict to one turn." }
            }),
            &["id"],
        ),

        tool(
            "thread_terminal",
            "Read the thread's persisted `bash` tool terminal output.",
            "GET",
            "/v1/threads/{id}/terminal",
            json!({
                "id": thread_id(),
                "turn_id": { "type": "string", "description": "Optional: restrict to one turn." }
            }),
            &["id"],
        ),

    ]);
}

fn threads_goal_todos(tools: &mut Vec<McpToolDoc>) {
    tools.extend([

        tool(
            "thread_goal_get",
            "Read the thread goal (Plan projection).",
            "GET",
            "/v1/threads/{id}/goal",
            json!({ "id": thread_id() }),
            &["id"],
        ),

        tool(
            "thread_goal_set",
            "Write the thread goal (appends a GoalUpdated authoritative event). Empty text is a structured 400.",
            "POST",
            "/v1/threads/{id}/goal",
            json!({
                "id": thread_id(),
                "goal": { "type": "string" }
            }),
            &["id", "goal"],
        ),

        tool(
            "thread_goal_clear",
            "Clear the thread goal (appends a GoalCleared event).",
            "DELETE",
            "/v1/threads/{id}/goal",
            json!({ "id": thread_id() }),
            &["id"],
        ),

        tool(
            "thread_todos_set",
            "Replace the thread todos wholesale (appends a TodosUpdated event; validated before the write).",
            "POST",
            "/v1/threads/{id}/todos",
            json!({
                "id": thread_id(),
                "body": dto_body("TodosRequest (todos: Vec<TodoEntry>; full replacement, not a delta)")
            }),
            &["id"],
        ),

        tool(
            "thread_todos_clear",
            "Clear the thread todos (appends a TodosCleared event).",
            "DELETE",
            "/v1/threads/{id}/todos",
            json!({ "id": thread_id() }),
            &["id"],
        ),

    ]);
}

fn turns(tools: &mut Vec<McpToolDoc>) {
    let turn_id = || json!({ "type": "string", "description": "Path parameter: turn id." });
    tools.extend([
        tool(
            "turn_start",
            "Start a real model turn on the thread (202 + TurnSummary). Only one turn per thread may run at a time; stream progress via thread_events.",
            "POST",
            "/v1/threads/{id}/turns",
            json!({
                "id": thread_id(),
                "body": dto_body("TurnRequest (text, optional attachment_ids already associated to this thread)")
            }),
            &["id"],
        ),
        tool(
            "turn_get",
            "Read one turn's summary and state.",
            "GET",
            "/v1/threads/{id}/turns/{turn_id}",
            json!({ "id": thread_id(), "turn_id": turn_id() }),
            &["id", "turn_id"],
        ),
        tool(
            "turn_steer",
            "Inject a steering instruction into the running turn; it enters model history at the next round boundary.",
            "POST",
            "/v1/threads/{id}/turns/{turn_id}/steer",
            json!({ "id": thread_id(), "turn_id": turn_id(), "body": dto_body("TurnRequest (text)") }),
            &["id", "turn_id"],
        ),
        tool(
            "turn_interrupt",
            "Interrupt the running turn; it converges to the Aborted terminal state.",
            "POST",
            "/v1/threads/{id}/turns/{turn_id}/interrupt",
            json!({ "id": thread_id(), "turn_id": turn_id() }),
            &["id", "turn_id"],
        ),
    ]);
}

fn gates(tools: &mut Vec<McpToolDoc>) {
    tools.extend([
        tool(
            "approval_resolve",
            "Resolve one pending approval. Read ids from thread_get's runtime_snapshot.pending_approvals.",
            "POST",
            "/v1/approvals/{id}",
            json!({
                "id": { "type": "string", "description": "Path parameter: approval id." },
                "body": dto_body("ResolveApprovalRequest (approved: bool)")
            }),
            &["id"],
        ),
        tool(
            "user_input_resolve",
            "Answer one pending user-input gate. Read ids from runtime_snapshot.pending_user_inputs.",
            "POST",
            "/v1/user-inputs/{id}",
            json!({
                "id": { "type": "string", "description": "Path parameter: request id." },
                "body": dto_body("ResolveUserInputRequest (answer: string)")
            }),
            &["id"],
        ),
    ]);
}

fn schedules(tools: &mut Vec<McpToolDoc>) {
    let schedule_id = || json!({ "type": "string", "description": "Path parameter: schedule id." });
    tools.extend([
        tool(
            "schedule_list",
            "List every scheduled task (most recently updated first) with run history and next_run_at.",
            "GET",
            "/v1/schedules",
            json!({}),
            &[],
        ),
        tool(
            "schedule_create",
            "Create a scheduled task (201). The tick loop runs it through the same start_turn validation path as manual turns; triggering also honours the runtime `schedules.enabled` switch.",
            "POST",
            "/v1/schedules",
            json!({
                "body": dto_body("CreateScheduleRequest (prompt, schedule{kind: interval|daily|weekly|monthly, ...}, optional title/workspace/autonomy/model/provider_id)")
            }),
            &["body"],
        ),
        tool(
            "schedule_update",
            "Merge an update onto a stored task: absent fields keep stored values; next_run_at is recomputed from now.",
            "PATCH",
            "/v1/schedules/{id}",
            json!({ "id": schedule_id(), "body": dto_body("UpdateScheduleRequest (clear optional text fields by sending an empty string)") }),
            &["id"],
        ),
        tool(
            "schedule_delete",
            "Delete the task and its run history. A missing id is a structured 404, not a silent success.",
            "DELETE",
            "/v1/schedules/{id}",
            json!({ "id": schedule_id() }),
            &["id"],
        ),
        tool(
            "schedule_run",
            "Run the task once, now (202): starts a real turn. Answers 409 while the task is running, the task is disabled (enable it first), or the schedules switch is off.",
            "POST",
            "/v1/schedules/{id}/run",
            json!({ "id": schedule_id() }),
            &["id"],
        ),
    ]);
}

fn memory(tools: &mut Vec<McpToolDoc>) {
    let memory_id = || json!({ "type": "string", "description": "Path parameter: memory id." });
    tools.extend([
        tool(
            "memory_list",
            "List long-term user memory (cross-session preferences/facts injected as a [user memory] system message each turn).",
            "GET",
            "/v1/memory",
            json!({}),
            &[],
        ),
        tool(
            "memory_create",
            "Add one long-term memory entry (16 KiB UTF-8 cap).",
            "POST",
            "/v1/memory",
            json!({ "body": dto_body("CreateMemoryRequest (text, optional tags/source)") }),
            &["body"],
        ),
        tool(
            "memory_update",
            "Update one long-term memory entry; absent fields keep stored values, and an explicit null source clears it (the only way to remove a stored source).",
            "PATCH",
            "/v1/memory/{id}",
            json!({ "id": memory_id(), "body": dto_body("UpdateMemoryRequest") }),
            &["id"],
        ),
        tool(
            "memory_delete",
            "Delete one long-term memory entry.",
            "DELETE",
            "/v1/memory/{id}",
            json!({ "id": memory_id() }),
            &["id"],
        ),
        tool(
            "memory_diagnostics",
            "Memory storage diagnostics (entry count, corrupt files).",
            "GET",
            "/v1/memory/diagnostics",
            json!({}),
            &[],
        ),
    ]);
}

fn attachments(tools: &mut Vec<McpToolDoc>) {
    let attachment_id =
        || json!({ "type": "string", "description": "Path parameter: attachment id (UUID)." });
    tools.extend([
        tool(
            "attachment_upload",
            "Upload an attachment (standard Base64 in `content`), returns 201 with metadata. Images reach the model only when the provider catalog explicitly declares supports_vision; other files are passed as preview_text.",
            "POST",
            "/v1/attachments",
            json!({
                "body": dto_body("AttachmentUploadRequest (original_filename, content: base64, optional mime_type/preview_text; a declared mime is magic-number checked, mismatch is 422)")
            }),
            &["body"],
        ),
        tool(
            "attachment_get",
            "Read attachment metadata (no bytes).",
            "GET",
            "/v1/attachments/{id}",
            json!({ "id": attachment_id() }),
            &["id"],
        ),
        tool(
            "attachment_delete",
            "Delete the attachment. Answers 409 (`attachment_in_use`) while any thread still references it.",
            "DELETE",
            "/v1/attachments/{id}",
            json!({ "id": attachment_id() }),
            &["id"],
        ),
        tool(
            "attachment_content",
            "Download the raw bytes (application/octet-stream, Content-Disposition: attachment).",
            "GET",
            "/v1/attachments/{id}/content",
            json!({ "id": attachment_id() }),
            &["id"],
        ),
        tool(
            "attachment_diagnostics",
            "Attachment storage diagnostics, including the orphan count (older than 30 days, unassociated).",
            "GET",
            "/v1/attachments/diagnostics",
            json!({}),
            &[],
        ),
        tool(
            "attachment_associate",
            "Associate an attachment with a thread (the authorization edge for turn references). Repeating the call does not append a second event; still 201.",
            "POST",
            "/v1/threads/{id}/attachments",
            json!({
                "id": thread_id(),
                "body": dto_body("AttachmentAssociationRequest (attachment_id)")
            }),
            &["id"],
        ),
    ]);
}

fn child_runs(tools: &mut Vec<McpToolDoc>) {
    let child_id = || json!({ "type": "string", "description": "Path parameter: child run id." });
    tools.extend([
        tool(
            "child_run_list",
            "List background child-run records. One row is one status transition; fold by child id (thread detail's child_ids is already folded).",
            "GET",
            "/v1/child-runs",
            json!({
                "thread_id": { "type": "string", "description": "Optional: filter by parent thread." }
            }),
            &[],
        ),
        tool(
            "child_run_get",
            "Read one child run's latest state.",
            "GET",
            "/v1/child-runs/{child_id}",
            json!({ "child_id": child_id() }),
            &["child_id"],
        ),
        tool(
            "child_run_delete",
            "Delete a finished child-run record. Running children answer 409.",
            "DELETE",
            "/v1/child-runs/{child_id}",
            json!({ "child_id": child_id() }),
            &["child_id"],
        ),
        tool(
            "child_run_abort",
            "Abort a child run.",
            "POST",
            "/v1/child-runs/{child_id}/abort",
            json!({ "child_id": child_id() }),
            &["child_id"],
        ),
        tool(
            "child_run_steer",
            "Steer a running child run.",
            "POST",
            "/v1/child-runs/{child_id}/steer",
            json!({ "child_id": child_id(), "body": dto_body("TurnRequest (text)") }),
            &["child_id"],
        ),
        tool(
            "child_run_resume",
            "Resume a finished child run, inheriting the recorded provider/model/autonomy.",
            "POST",
            "/v1/child-runs/{child_id}/resume",
            json!({ "child_id": child_id(), "body": dto_body("ResumeChildRequest (optional text)") }),
            &["child_id"],
        ),
        tool(
            "child_run_thread",
            "Read the child run's inline sub-thread id.",
            "GET",
            "/v1/child-runs/{child_id}/thread",
            json!({ "child_id": child_id() }),
            &["child_id"],
        ),
        tool(
            "child_run_events",
            "Subscribe to the child run's inline sub-thread event stream (SSE, same semantics as thread_events).",
            "GET",
            "/v1/child-runs/{child_id}/events",
            json!({
                "child_id": child_id(),
                "since_seq": { "type": "integer", "minimum": 0 }
            }),
            &["child_id"],
        ),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_atomic_tools_are_registered() {
        let catalog = tools();
        let names: Vec<&str> = catalog.iter().map(|tool| tool.name).collect();
        for expected in [
            "thread_create",
            "thread_list",
            "thread_get",
            "schedule_create",
            "schedule_list",
            "turn_start",
            "turn_interrupt",
            "attachment_upload",
            "child_run_list",
        ] {
            assert!(names.contains(&expected), "missing tool: {expected}");
        }
    }

    #[test]
    fn dto_body_references_name_the_contract_type() {
        let catalog = tools();
        let start = catalog
            .iter()
            .find(|tool| tool.name == "turn_start")
            .expect("turn_start");
        let description = start.input_schema["properties"]["body"]["description"]
            .as_str()
            .expect("description");
        assert!(description.contains("TurnRequest"), "{description}");
    }
}
