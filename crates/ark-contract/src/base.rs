//! Base platform contracts: providers, skills, workspace, MCP servers.
//!
//! These four are *platform* concerns, not plugin concerns (design 7.5.4). The
//! shapes here are the wire shapes the GUI and the runtime agree on, so they
//! live in the contract crate next to `RuntimeEvent` rather than inside the
//! service crates that happen to implement them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Model providers
// ---------------------------------------------------------------------------

/// One configured model provider.
///
/// `api_key` is only ever an *input*: read paths go through [`ProviderView`],
/// which reports `has_api_key` and a masked hint instead of the secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ProviderEntry {
    /// Stable id, e.g. `deepseek` or `qwen-local`.
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    /// `openai_chat` | `openai_responses` | `anthropic_messages`.
    #[serde(default)]
    pub endpoint_format: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
}

fn default_true() -> bool {
    true
}

impl ProviderEntry {
    pub fn new(id: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: None,
            base_url: String::new(),
            api_key: None,
            endpoint_format: String::new(),
            model: model.into(),
            enabled: true,
            headers: BTreeMap::new(),
            max_tokens: None,
            temperature: None,
        }
    }
}

/// Redacted provider as returned by `GET /v1/runtime/model-providers`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ProviderView {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub base_url: String,
    pub endpoint_format: String,
    pub model: String,
    pub enabled: bool,
    pub is_default: bool,
    pub has_api_key: bool,
    /// First and last four characters only, e.g. `sk-a...cdef`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_hint: Option<String>,
}

/// `GET /v1/runtime/model-providers`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ProviderListResponse {
    pub providers: Vec<ProviderView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider_id: Option<String>,
    /// Provider id the running process is actually bound to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_provider_id: Option<String>,
}

/// `POST /v1/runtime/model-providers/test`.
///
/// Either reference a saved provider by id, or test an unsaved draft. Testing a
/// draft is what lets the settings dialog validate before saving.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ProviderTestRequest {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub entry: Option<ProviderEntry>,
}

/// Result of one connectivity probe.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ProviderTestResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[ts(as = "f64")]
    pub latency_ms: u64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

/// `triggers` block of a `SKILL.md` front matter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct SkillTriggers {
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub prompt_patterns: Vec<String>,
    #[serde(default)]
    pub file_types: Vec<String>,
}

/// Parsed skill manifest (design appendix: skill runtime).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct SkillManifest {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default)]
    pub triggers: SkillTriggers,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default)]
    pub priority: i32,
}

fn default_entry() -> String {
    "SKILL.md".to_string()
}

/// Where a skill came from. A thread must not be able to discover another
/// thread's extras, so the origin travels with the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Runtime-wide root, visible to every thread.
    Base,
    /// Configured on one thread only.
    ThreadExtra,
}

/// One discovered skill, as listed to the GUI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct SkillSummary {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub version: String,
    pub root: String,
    pub entry_path: String,
    pub priority: i32,
    pub source: SkillSource,
    /// True when the directory had no manifest and was read as a bare SKILL.md.
    pub legacy: bool,
}

/// A recoverable per-root load failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct SkillLoadWarning {
    pub root: String,
    pub message: String,
}

/// Runtime-wide skill diagnostics (`GET /v1/skills`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct SkillDiagnostics {
    pub enabled: bool,
    pub roots: Vec<String>,
    pub skills: Vec<SkillSummary>,
    pub validation_errors: Vec<SkillLoadWarning>,
}

/// Per-thread skill view (`GET /v1/threads/{id}/skills`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ThreadSkillsResponse {
    pub enabled: bool,
    pub base_roots: Vec<String>,
    pub extra_roots: Vec<String>,
    pub loaded_extra_roots: Vec<String>,
    pub base_skills: Vec<SkillSummary>,
    pub extra_skills: Vec<SkillSummary>,
    pub warnings: Vec<SkillLoadWarning>,
    pub base_validation_errors: Vec<SkillLoadWarning>,
}

/// Why a skill was activated for a turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct SkillActivation {
    pub skill_id: String,
    pub reason: String,
    pub score: i32,
}

/// What the turn actually gets from the skill runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct SkillTurnResolution {
    pub active_skill_ids: Vec<String>,
    pub activations: Vec<SkillActivation>,
    pub instructions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tool_names: Option<Vec<String>>,
    pub injected_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

/// `GET /v1/workspace/status?path=...`.
///
/// Field names match the existing runtime exactly, so the renderer keeps
/// working unchanged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct WorkspaceStatus {
    pub path: String,
    pub exists: bool,
    pub is_git_repository: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_dirty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub file_change_count: Option<u64>,
    pub checked_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// External MCP servers
// ---------------------------------------------------------------------------

/// How to reach an external MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportKind {
    Stdio,
    Sse,
    StreamableHttp,
}

impl McpTransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            McpTransportKind::Stdio => "stdio",
            McpTransportKind::Sse => "sse",
            McpTransportKind::StreamableHttp => "streamable_http",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "stdio" => Some(McpTransportKind::Stdio),
            "sse" => Some(McpTransportKind::Sse),
            "streamable_http" | "streamable-http" | "http" => {
                Some(McpTransportKind::StreamableHttp)
            }
            _ => None,
        }
    }
}

/// One configured external MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct McpServerEntry {
    pub id: String,
    pub transport: McpTransportKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `stdio`: executable to spawn.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// `sse` / `streamable_http`: endpoint URL.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Tools the user allows this server to contribute; empty means all.
    #[serde(default)]
    pub allow_tools: Vec<String>,
}

/// Server state as reported to the GUI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct McpServerView {
    pub id: String,
    pub transport: McpTransportKind,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub allow_tools: Vec<String>,
    /// Secret values are never returned; these flags only tell an editor that
    /// leaving its blank secret inputs untouched will preserve existing values.
    pub has_env: bool,
    pub has_headers: bool,
    pub connected: bool,
    #[ts(as = "f64")]
    pub tool_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// `GET /v1/mcp/servers`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct McpServerListResponse {
    pub servers: Vec<McpServerView>,
}

/// Result of connecting to one server and listing its tools.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct McpServerTestResponse {
    pub ok: bool,
    pub tools: Vec<String>,
    #[ts(as = "f64")]
    pub latency_ms: u64,
    pub message: String,
}
