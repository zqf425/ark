//! ark-memory-plugin: a long-term memory store that uses only the plugin host's
//! open surface.
//!
//! ## Why this crate exists
//!
//! `ark-store` already owns *user-maintained* memory (`memory.json`, injected as
//! `[user memory]`). That memory is hand-written and deliberately small. This
//! plugin is the other half: a **model-facing** store the agent writes to during
//! a turn and reads back later, kept in the plugin's own isolated root.
//!
//! ## Coupling contract
//!
//! The dependency list is the contract (see `docs/modules/ark-memory-plugin.md`):
//! `ark-common`, `ark-ports`, `ark-plugin`, `ark-mcp` and serialisation. This
//! crate must never depend on `apps/arkd`, `ark-agent`, `ark-store` or `ark-llm`.
//! It reaches storage only through the [`ark_ports`] view traits the host binds
//! to one plugin id, so it cannot address another plugin's keys or files.
//!
//! ## Not in the default build
//!
//! The crate is a workspace member so it compiles and its tests run, but
//! `arkd` never references it: a plugin is an opt-in install, and nothing in the
//! runtime startup path pulls this code in.

#![deny(clippy::all)]

mod store;
mod tools;

pub use store::{MemoryRecord, MemoryStore, MAX_TEXT_CHARS};
pub use tools::{DeleteTool, ListTool, SearchTool, StoreTool};

use std::sync::Arc;

use ark_common::Result;
use ark_mcp::{ExposureHandle, McpHandler};
use ark_plugin::service::{Plugin, PluginContext, Registration};
use ark_plugin::PluginManifest;
use ark_ports::McpToolSpec;

/// Plugin id. Must match `plugin.json`; the host rejects a spec that names a
/// different plugin than the context it came from.
pub const PLUGIN_ID: &str = "ark.memory.core";

/// The plugin, as the host sees it.
///
/// Implementing [`Plugin`] is the whole integration surface: the host calls
/// `manifest()` before running any of our code, then hands a `PluginContext`
/// bound to our id to `register()`. Nothing here can be constructed with
/// another plugin's identity, because [`ark_plugin::PluginIdentity`] has no
/// public constructor.
pub struct MemoryPlugin {
    manifest: PluginManifest,
}

impl MemoryPlugin {
    /// Build the plugin from its checked-in `plugin.json`.
    ///
    /// Parsed through [`ark_plugin::DirectoryManifest`], the same type the host
    /// uses for a folder on disk, then converted with `to_manifest()`. Parsing
    /// here (rather than trusting a Rust literal) is what keeps the shipped
    /// manifest and the crate from drifting: a manifest the host would reject
    /// fails at construction instead of at load.
    pub fn from_manifest_json(raw: &str) -> Result<Self> {
        let directory: ark_plugin::DirectoryManifest =
            serde_json::from_str(raw).map_err(|err| {
                ark_common::ArkError::validation(format!("invalid plugin.json: {err}"))
            })?;
        let manifest = directory.to_manifest()?;
        manifest.validate()?;
        Ok(Self { manifest })
    }

    /// The manifest this crate ships (`plugin.json`).
    pub const MANIFEST_JSON: &'static str = include_str!("../plugin.json");
}

impl Plugin for MemoryPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn register(&self, ctx: &mut PluginContext) -> Result<Registration> {
        let exposure = publish(ctx)?;
        // The registration owns the exposure handle: dropping it withdraws every
        // tool, which is the host's documented unload path.
        let mut registration = Registration::new();
        registration.push_exposure(exposure);
        Ok(registration)
    }
}

/// Publish the four tools on `ctx` and return the handle that withdraws them.
/// Separate from [`Plugin::register`] so a test can drive it without a
/// `Registration`, and so the handle's lifetime stays explicit.
pub fn publish(ctx: &PluginContext) -> Result<ExposureHandle> {
    // `ctx.kv()` is `ScopedKv`, which implements `KvService`; the store takes
    // both handles as trait objects so it holds no dependency on the concrete
    // scoped types. The `Arc::clone` → unsize step is written out because
    // `Arc::clone(&Arc<ScopedKv>)` would clone the concrete handle first.
    let kv_handle: Arc<ark_plugin::service::ScopedKv> = Arc::clone(ctx.kv());
    let kv: Arc<dyn ark_ports::KvService> = kv_handle;
    let store = Arc::new(MemoryStore::new(Arc::clone(ctx.files()), kv));
    // The KV index is a cache, so a fresh process must rebuild it from the
    // plugin's file root before anything reads. A failure here is not fatal:
    // `list`/`search` degrade to an empty result rather than refusing to load.
    if let Err(error) = store.rebuild_index() {
        ctx.log().log(ark_ports::services::LogRecord {
            plugin: PLUGIN_ID.to_string(),
            level: "warn".to_string(),
            message: format!("memory index rebuild failed: {}", error.message),
            fields: Default::default(),
        });
    }

    let specs: Vec<(McpToolSpec, Arc<dyn McpHandler>)> = vec![
        (
            spec(
                "memory_store",
                "Store one long-term memory. Use it for durable facts the user \
                 stated or asked to remember; do not store transient turn context.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "The memory to keep." },
                        "tags": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional lowercase tags for later filtering."
                        },
                        "source": { "type": "string", "description": "Where it came from, e.g. `manual`." }
                    },
                    "required": ["text"],
                    "additionalProperties": false
                }),
            ),
            Arc::new(StoreTool::new(Arc::clone(&store))),
        ),
        (
            spec(
                "memory_search",
                "Search stored memories by keyword and optional tags.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Case-insensitive substring to match." },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
            ),
            Arc::new(SearchTool::new(Arc::clone(&store))),
        ),
        (
            spec(
                "memory_list",
                "List stored memories, newest first.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                    },
                    "additionalProperties": false
                }),
            ),
            Arc::new(ListTool::new(Arc::clone(&store))),
        ),
        (
            spec(
                "memory_delete",
                "Delete one memory by id.",
                serde_json::json!({
                    "type": "object",
                    "properties": { "id": { "type": "string" } },
                    "required": ["id"],
                    "additionalProperties": false
                }),
            ),
            Arc::new(DeleteTool::new(store)),
        ),
    ];

    ctx.expose_tool_handlers(specs)
}

fn spec(tool: &str, description: &str, schema: serde_json::Value) -> McpToolSpec {
    McpToolSpec {
        plugin_id: PLUGIN_ID.to_string(),
        tool: tool.to_string(),
        description: description.to_string(),
        schema,
    }
}

#[cfg(test)]
mod tests;
