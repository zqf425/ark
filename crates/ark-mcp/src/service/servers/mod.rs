//! External MCP server connections: configuration, persistence and tool export.
//!
//! # What this module owns
//!
//! [`McpServerRegistry`] is the user-facing half of the MCP client direction. The
//! in-process [`crate::service::McpBus`] is unchanged: plugins still publish
//! tools locally and this crate is still the single egress. Here we add the
//! *other* direction, where the runtime is the client of servers the user
//! configured (design 7.6.1).
//!
//! ```text
//! <data_dir>/mcp-servers.json -> entries -> transport -> McpToolAdapter
//!                                   ^                          |
//!                                   |                          v
//!                            list()/upsert()           ark_ports::Tool
//! ```
//!
//! # Failure isolation
//!
//! Nothing here treats one server as fatal. [`McpServerRegistry::connect_all`]
//! tries each enabled server independently and records `last_error` on the view
//! when it fails, so a broken `command` or an unreachable endpoint cannot hide
//! the servers that do work. [`McpServerRegistry::test`] never panics either: a
//! spawn failure, a timeout and an HTTP error status all come back as
//! `ok: false` with a message the GUI can show verbatim.
//!
//! # Secrets
//!
//! v1 persists `env` / `headers` in the JSON file as-is and never echoes them:
//! [`ark_contract::McpServerView`] has no field for a value, so `list()` cannot
//! leak one. Moving the values into the OS keyring is a follow-up; see
//! [`store`] for the note kept next to the write path.

mod egress;
mod session;
mod store;
mod tool;

#[cfg(test)]
mod tests;

use ark_common::{ArkError, Result};
use ark_contract::{McpServerEntry, McpServerListResponse, McpServerTestResponse, McpServerView};
use ark_http::{HttpBus, HttpPolicy, MemoryDiagnostics};
use ark_ports::Tool;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub use tool::McpToolAdapter;

use crate::domain::server_entry;
use session::{BusSlot, Connected, ServerSession};
use store::ServerStore;

/// Upper bound for `test()` and for one server during `connect_all()`.
pub(crate) const CONNECT_TIMEOUT_MS: u64 = 15_000;

/// Per-server connection state. Never persisted: a restart reconnects.
#[derive(Default)]
struct Runtime {
    session: Option<Arc<ServerSession>>,
    connected: bool,
    tool_count: u64,
    last_error: Option<String>,
}

/// Everything behind the lock. The guard is never held across an `.await`;
/// async steps clone what they need first.
#[derive(Default)]
struct State {
    /// Sorted by id, matching the order returned to the GUI.
    entries: Vec<McpServerEntry>,
    /// Keyed by server id. A missing key means "never connected".
    runtime: BTreeMap<String, Runtime>,
}

/// Configured external MCP servers plus their live connections.
pub struct McpServerRegistry {
    store: ServerStore,
    /// Shared HTTP egress. `Err` carries why it could not be built, so a
    /// stdio-only configuration keeps working on a host where HTTP is broken.
    bus: BusSlot,
    state: Mutex<State>,
}

impl std::fmt::Debug for McpServerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerRegistry")
            .field("path", &self.store.path())
            .field("bus", &self.bus.is_ok())
            .finish_non_exhaustive()
    }
}

impl McpServerRegistry {
    /// Load `<data_dir>/mcp-servers.json`. A missing document is an empty
    /// configuration, never a startup error.
    pub fn new(data_dir: PathBuf) -> Self {
        let registry = Self {
            store: ServerStore::new(&data_dir),
            bus: build_bus(),
            state: Mutex::new(State::default()),
        };
        let entries = registry.store.load();
        let mut sorted = entries;
        sorted.sort_by(|a, b| a.id.cmp(&b.id));
        if let Ok(mut state) = registry.state.lock() {
            state.entries = sorted;
        }
        registry
    }

    /// Path of the persistence file (useful for diagnostics and tests).
    pub fn data_path(&self) -> &std::path::Path {
        self.store.path()
    }

    /// Current configuration and connection state. Never contains `env` or
    /// header values: [`McpServerView`] has no field for them.
    pub fn list(&self) -> McpServerListResponse {
        match self.state.lock() {
            Ok(state) => views(&state),
            Err(_) => McpServerListResponse::default(),
        }
    }

    /// Insert or replace one server.
    ///
    /// Re-using an existing id is an update, not an error. The document is
    /// written before memory changes, so a failed write cannot leave the
    /// in-memory list ahead of the file. A replaced entry loses its session,
    /// because the old transport was built from the old configuration.
    pub fn upsert(&self, entry: McpServerEntry) -> Result<McpServerListResponse> {
        let mut entry = server_entry::normalize(entry)?;
        let mut state = self.lock()?;
        let mut next = state.entries.clone();
        match next.iter_mut().find(|existing| existing.id == entry.id) {
            Some(slot) => {
                // The read model never exposes env/header values. An edit that
                // omits them must keep the stored secrets rather than erase
                // credentials the form intentionally could not prefill.
                if entry.env.is_empty() {
                    entry.env = slot.env.clone();
                }
                if entry.headers.is_empty() {
                    entry.headers = slot.headers.clone();
                }
                *slot = entry.clone();
            }
            None => next.push(entry.clone()),
        }
        next.sort_by(|a, b| a.id.cmp(&b.id));
        ensure_distinct_egress(&next)?;
        self.store.save(&next)?;
        state.entries = next;
        state.runtime.remove(&entry.id);
        Ok(views(&state))
    }

    /// Remove one server. An unknown id is a `NotFound` error, not a silent
    /// success: the GUI needs to tell "already gone" from "never existed".
    pub fn delete(&self, id: &str) -> Result<McpServerListResponse> {
        let id = id.trim();
        let mut state = self.lock()?;
        if !state.entries.iter().any(|entry| entry.id == id) {
            return Err(
                ArkError::not_found(format!("mcp server {id} is not configured"))
                    .with_detail("mcp_server_not_found"),
            );
        }
        let next: Vec<McpServerEntry> = state
            .entries
            .iter()
            .filter(|entry| entry.id != id)
            .cloned()
            .collect();
        self.store.save(&next)?;
        state.entries = next;
        state.runtime.remove(id);
        Ok(views(&state))
    }

    /// Connect to one server, list its tools, and keep the session.
    ///
    /// Disabled servers may still be tested: the user is validating a
    /// configuration before turning it on.
    pub async fn test(&self, id: &str) -> McpServerTestResponse {
        let entry = match self.entry(id) {
            Some(entry) => entry,
            None => return missing(id),
        };
        let started = Instant::now();
        let outcome = connect_entry(&entry, &self.bus).await;
        let latency_ms = elapsed_ms(started);
        match outcome {
            Ok(connected) => {
                let tools: Vec<String> = connected
                    .tools
                    .iter()
                    .filter(|tool| allowed(&entry, &tool.name))
                    .map(|tool| qualified(&entry.id, &tool.name))
                    .collect();
                self.commit_connected(&entry.id, &connected, tools.len() as u64);
                McpServerTestResponse {
                    message: format!(
                        "connected to {} and found {} tool(s)",
                        entry.id,
                        tools.len()
                    ),
                    ok: true,
                    tools,
                    latency_ms,
                }
            }
            Err(err) => {
                let message = describe(&err);
                self.record_error(&entry.id, message.clone());
                McpServerTestResponse {
                    ok: false,
                    tools: Vec::new(),
                    latency_ms,
                    message,
                }
            }
        }
    }

    /// Connect every enabled server and return the adapters that came up.
    ///
    /// Each server is attempted on its own: a failure records `last_error` and
    /// the loop continues, so one unspawnable command cannot cost the user the
    /// other servers' tools.
    pub async fn connect_all(&self) -> Vec<Arc<dyn Tool>> {
        let entries = self.enabled_entries();
        self.reset_disabled();
        let mut adapters: Vec<Arc<dyn Tool>> = Vec::new();
        for entry in entries {
            match connect_entry(&entry, &self.bus).await {
                Ok(connected) => {
                    let exported = export(&entry, &connected);
                    self.commit_connected(&entry.id, &connected, exported.len() as u64);
                    adapters.extend(exported);
                }
                Err(err) => self.record_error(&entry.id, describe(&err)),
            }
        }
        adapters
    }

    fn entry(&self, id: &str) -> Option<McpServerEntry> {
        let id = id.trim();
        self.state
            .lock()
            .ok()?
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
    }

    fn enabled_entries(&self) -> Vec<McpServerEntry> {
        self.state
            .lock()
            .map(|state| {
                state
                    .entries
                    .iter()
                    .filter(|entry| entry.enabled)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn commit_connected(&self, id: &str, connected: &Connected, tool_count: u64) {
        if let Ok(mut state) = self.state.lock() {
            let runtime = state.runtime.entry(id.to_string()).or_default();
            runtime.session = Some(connected.session.clone());
            runtime.connected = true;
            runtime.tool_count = tool_count;
            runtime.last_error = None;
        }
    }

    fn record_error(&self, id: &str, message: String) {
        if let Ok(mut state) = self.state.lock() {
            let runtime = state.runtime.entry(id.to_string()).or_default();
            runtime.session = None;
            runtime.connected = false;
            runtime.tool_count = 0;
            runtime.last_error = Some(message);
        }
    }

    /// A disabled server keeps its entry but loses its connection.
    fn reset_disabled(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let disabled: Vec<String> = state
            .entries
            .iter()
            .filter(|entry| !entry.enabled)
            .map(|entry| entry.id.clone())
            .collect();
        for id in disabled {
            state.runtime.remove(&id);
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| ArkError::internal("mcp server registry poisoned"))
    }
}

/// Connect with the shared timeout and keep the session for tool execution.
async fn connect_entry(entry: &McpServerEntry, bus: &BusSlot) -> Result<Connected> {
    let attempt = session::connect(entry, bus);
    match tokio::time::timeout(Duration::from_millis(CONNECT_TIMEOUT_MS), attempt).await {
        Ok(result) => result,
        Err(_) => Err(ArkError::new(
            ark_common::ArkErrorCode::RuntimeUnavailable,
            format!(
                "mcp server {} did not answer within {CONNECT_TIMEOUT_MS}ms",
                entry.id
            ),
        )
        .with_detail("mcp_connect_timeout")),
    }
}

/// Build adapters, honouring `allow_tools` (empty = everything).
fn export(entry: &McpServerEntry, connected: &Connected) -> Vec<Arc<dyn Tool>> {
    connected
        .tools
        .iter()
        .filter(|tool| allowed(entry, &tool.name))
        .map(|tool| {
            Arc::new(McpToolAdapter::new(
                &entry.id,
                &tool.name,
                Some(tool.input_schema.clone()),
                connected.session.clone(),
            )) as Arc<dyn Tool>
        })
        .collect()
}

fn allowed(entry: &McpServerEntry, tool: &str) -> bool {
    entry.allow_tools.is_empty() || entry.allow_tools.iter().any(|item| item == tool)
}

/// `<server>.<tool>`, the name the model uses and the GUI lists.
fn qualified(server: &str, tool: &str) -> String {
    format!("{server}.{tool}")
}

fn views(state: &State) -> McpServerListResponse {
    McpServerListResponse {
        servers: state
            .entries
            .iter()
            .map(|entry| view_of(entry, state.runtime.get(&entry.id)))
            .collect(),
    }
}

fn view_of(entry: &McpServerEntry, runtime: Option<&Runtime>) -> McpServerView {
    McpServerView {
        id: entry.id.clone(),
        transport: entry.transport,
        enabled: entry.enabled,
        command: entry.command.clone(),
        args: entry.args.clone(),
        url: entry.url.clone(),
        allow_tools: entry.allow_tools.clone(),
        has_env: !entry.env.is_empty(),
        has_headers: !entry.headers.is_empty(),
        connected: runtime.map(|state| state.connected).unwrap_or(false),
        tool_count: runtime.map(|state| state.tool_count).unwrap_or(0),
        last_error: runtime.and_then(|state| state.last_error.clone()),
    }
}

fn missing(id: &str) -> McpServerTestResponse {
    McpServerTestResponse {
        ok: false,
        tools: Vec::new(),
        latency_ms: 0,
        message: format!("mcp server {} is not configured", id.trim()),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

/// Two entries must not share one HTTP egress grant.
///
/// The grant is derived from the server id and lives in one namespace, so ids
/// that differ only by case would silently overwrite each other's allowlist.
fn ensure_distinct_egress(entries: &[McpServerEntry]) -> Result<()> {
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for entry in entries {
        let derived = egress::egress_plugin_id(&entry.id);
        if let Some(existing) = seen.get(&derived) {
            if *existing != entry.id {
                return Err(ArkError::conflict(format!(
                    "mcp server ids {existing:?} and {:?} only differ by case; rename one of them",
                    entry.id
                ))
                .with_detail("mcp_server_id_conflict"));
            }
        }
        seen.insert(derived, entry.id.as_str());
    }
    Ok(())
}

/// Turn an internal error into a message aimed at the user.
///
/// `detail` discriminates the cases the GUI cares about - spawn failure,
/// timeout, HTTP status - without exposing a Rust backtrace.
fn describe(err: &ArkError) -> String {
    match err.detail.as_deref() {
        Some("mcp_transport_spawn_failed") => {
            format!("failed to start the server: {}", err.message)
        }
        Some("mcp_connect_timeout") => err.message.clone(),
        Some("mcp_transport_status") => format!("server rejected the request: {}", err.message),
        Some("mcp_egress_unavailable") => format!("http egress unavailable: {}", err.message),
        Some("invalid_command") | Some("invalid_url") => err.message.clone(),
        _ => err.message.clone(),
    }
}

/// One HTTP egress for every remote server.
///
/// `allow_insecure_http` is on because a user may configure a server on
/// `localhost`; the per-server allowlist still limits each entry to its own
/// host, so this is not a general downgrade.
fn build_bus() -> BusSlot {
    let policy = HttpPolicy {
        allow_insecure_http: true,
        ..HttpPolicy::default()
    };
    HttpBus::new(
        policy,
        Arc::new(ark_http::CredentialVault::new()),
        Arc::new(MemoryDiagnostics::new(256)),
    )
    .map(Arc::new)
    .map_err(|err| format!("cannot build mcp http egress: {}", err.message))
}
