//! Platform-management tools: providers, workspace, external MCP servers,
//! runtime observation and admin routes. Split from `mcp_tools` so the catalog
//! file stays well under the 1000-line file limit as families grow.

use serde_json::{json, Value};

use super::{dto_body, local_body, tool, McpToolDoc};

/// Path parameter: provider id.
fn provider_id() -> Value {
    json!({ "type": "string", "description": "Path parameter: provider id." })
}

/// Path parameter: MCP server id.
fn server_id() -> Value {
    json!({ "type": "string", "description": "Path parameter: MCP server id." })
}

fn providers(tools: &mut Vec<McpToolDoc>) {
    tools.extend([
        tool(
            "provider_list",
            "List model providers. API keys are only ever returned as presence flags and mask hints, never plaintext.",
            "GET",
            "/v1/runtime/providers",
            json!({}),
            &[],
        ),
        tool(
            "provider_upsert",
            "Insert or update a provider (empty api key/header keeps the stored secret); make_default=true also selects it as default.",
            "POST",
            "/v1/runtime/providers",
            json!({ "body": local_body("UpsertProviderBody = ProviderEntry flattened + make_default:boolean") }),
            &["body"],
        ),
        tool(
            "provider_test",
            "Probe provider connectivity (a saved id or an unsaved draft entry).",
            "POST",
            "/v1/runtime/providers/test",
            json!({ "body": dto_body("ProviderTestRequest (provider_id or entry)") }),
            &["body"],
        ),
        tool(
            "provider_models",
            "Query the provider model catalog (bounded, sorted, deduplicated; never echoes secrets or URLs carrying them).",
            "POST",
            "/v1/runtime/providers/models",
            json!({ "body": dto_body("ProviderModelCatalogRequest") }),
            &["body"],
        ),
        tool(
            "provider_delete",
            "Delete a provider; deleting the default clears the default.",
            "DELETE",
            "/v1/runtime/providers/{id}",
            json!({ "id": provider_id() }),
            &["id"],
        ),
        tool(
            "provider_set_default",
            "Make an existing, enabled provider the default.",
            "POST",
            "/v1/runtime/providers/{id}/default",
            json!({ "id": provider_id() }),
            &["id"],
        ),
        tool(
            "skill_list",
            "List global skills (diagnostic view: loaded, duplicate slugs downgraded, available ids).",
            "GET",
            "/v1/skills",
            json!({}),
            &[],
        ),
        tool(
            "plugins_report",
            "Read the local plugin load report (loaded/failed/skipped/diagnostics/HTTP denials). Read-only.",
            "GET",
            "/v1/runtime/plugins",
            json!({}),
            &[],
        ),
    ]);
}

fn workspace(tools: &mut Vec<McpToolDoc>) {
    tools.extend([
        tool(
            "workspace_view",
            "Read the active workspace view (canonical path and generation).",
            "GET",
            "/v1/workspace",
            json!({}),
            &[],
        ),
        tool(
            "workspace_status",
            "Read the workspace git status (each call is hard-capped at 3s so a wedged git cannot stall the request).",
            "GET",
            "/v1/workspace/status",
            json!({}),
            &[],
        ),
        tool(
            "workspace_switch",
            "Switch the active workspace. Answers 409 while a parent or child run is active; nothing is implicitly cancelled.",
            "POST",
            "/v1/workspace/switch",
            json!({ "body": dto_body("SwitchWorkspaceRequest (path)") }),
            &["body"],
        ),
        tool(
            "workspace_diff",
            "Read the workspace git diff (carries a generation; discard responses whose generation changed).",
            "GET",
            "/v1/workspace/diff",
            json!({}),
            &[],
        ),
        tool(
            "workspace_validate",
            "Validate that a directory exists and return its normalized path, without switching.",
            "POST",
            "/v1/workspace/validate",
            json!({ "body": local_body("ValidateWorkspaceBody (path)") }),
            &["body"],
        ),
    ]);
}

fn mcp_servers(tools: &mut Vec<McpToolDoc>) {
    tools.extend([
        tool(
            "mcp_server_list",
            "List configured external MCP servers (safe read view: env/header values are never returned).",
            "GET",
            "/v1/mcp/servers",
            json!({}),
            &[],
        ),
        tool(
            "mcp_server_upsert",
            "Insert or update an external MCP server (stdio / streamable_http); success re-discovers tools and atomically replaces the `mcp` tool group.",
            "POST",
            "/v1/mcp/servers",
            json!({ "body": dto_body("McpServerEntry (empty env/header keeps stored secrets)") }),
            &["body"],
        ),
        tool(
            "mcp_server_delete",
            "Delete an external MCP server and drop its tools from the `mcp` group.",
            "DELETE",
            "/v1/mcp/servers/{id}",
            json!({ "id": server_id() }),
            &["id"],
        ),
        tool(
            "mcp_server_test",
            "Test an external MCP server connection without changing published tools.",
            "POST",
            "/v1/mcp/servers/{id}/test",
            json!({ "id": server_id() }),
            &["id"],
        ),
    ]);
}

fn runtime(tools: &mut Vec<McpToolDoc>) {
    tools.extend([
        tool(
            "runtime_info",
            "Read runtime info (version, started_at, data dir).",
            "GET",
            "/v1/runtime/info",
            json!({}),
            &[],
        ),
        tool(
            "runtime_tools",
            "List everything currently registered on the model tool table (builtins, plugin tools, external MCP tools).",
            "GET",
            "/v1/runtime/tools",
            json!({}),
            &[],
        ),
        tool(
            "runtime_capabilities",
            "Read the runtime capability catalog (tool/model_provider/ui_page/ui_panel plus the page catalog).",
            "GET",
            "/v1/runtime/capabilities",
            json!({}),
            &[],
        ),
        tool(
            "usage_total",
            "Read the process-wide model usage totals.",
            "GET",
            "/v1/usage",
            json!({}),
            &[],
        ),
        tool(
            "llm_rounds",
            "Diagnostic projection of every model round from authoritative usage events (no prompts, keys or user prose).",
            "GET",
            "/v1/debug/llm-rounds",
            json!({
                "thread_id": { "type": "string", "description": "Optional: restrict to one thread." },
                "limit": { "type": "integer", "minimum": 1 },
                "offset": { "type": "integer", "minimum": 0 }
            }),
            &[],
        ),
    ]);
}

fn admin(tools: &mut Vec<McpToolDoc>) {
    tools.extend([
        tool(
            "config_view",
            "Read the runtime configuration view (validated snapshot and load diagnostics, no secrets).",
            "GET",
            "/v1/runtime/config",
            json!({}),
            &[],
        ),
        tool(
            "config_apply",
            "Apply a config patch. Fields classified `Restart` reject the whole patch; `Hot` fields take effect immediately.",
            "POST",
            "/v1/runtime/config",
            json!({ "body": dto_body("ConfigPatch (optional overrides per config section)") }),
            &["body"],
        ),
        tool(
            "subagents_view",
            "Read subagent settings (enabled / auto_continue / max_concurrency).",
            "GET",
            "/v1/subagents/config",
            json!({}),
            &[],
        ),
        tool(
            "subagents_configure",
            "Hot-update subagent settings (same ApplyResult semantics as config_apply).",
            "POST",
            "/v1/subagents/config",
            json!({ "body": dto_body("SubagentSettings (auto_continue and default_background default to true; set either false to disable automatic synthesis or non-blocking dispatch)") }),
            &["body"],
        ),
        tool(
            "session_resume_thread",
            "Resolve a legacy session id into the full thread detail (this runtime has exactly one durable session object: the thread).",
            "POST",
            "/v1/sessions/{id}/resume-thread",
            json!({ "id": { "type": "string", "description": "Path parameter: session id (a thread id)." } }),
            &["id"],
        ),
        tool(
            "shutdown",
            "Request graceful shutdown: cancel active turns and pending continuations, converge every terminal state, then exit.",
            "POST",
            "/v1/shutdown",
            json!({}),
            &[],
        ),
    ]);
}

/// All platform-family tools.
pub(super) fn tools(tools: &mut Vec<McpToolDoc>) {
    providers(tools);
    workspace(tools);
    mcp_servers(tools);
    runtime(tools);
    admin(tools);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_family_is_complete() {
        let mut catalog = Vec::new();
        tools(&mut catalog);
        let names: Vec<&str> = catalog.iter().map(|tool| tool.name).collect();
        for expected in [
            "provider_upsert",
            "workspace_switch",
            "mcp_server_test",
            "runtime_capabilities",
            "shutdown",
        ] {
            assert!(
                names.contains(&expected),
                "missing platform tool: {expected}"
            );
        }
    }
}
