//! Governed provision flow for directory plugins
//! (requirements/plugin-provisioning.md).
//!
//! A directory plugin may declare a `provision` section: how to detect its
//! tool, how to install it, and which external MCP server to register once
//! enabled. The plugin contributes declarations only — every step runs
//! host-side, spawning a bare binary name (no shell, no path), with a cleared
//! environment, bounded time and bounded output. Lifecycle state persists in
//! `<plugin_dir>/provision-state.json`; boot re-runs detection so a vanished
//! binary downgrades a stale `enabled` to `needs_install`.
//!
//! State machine: `disabled → (enable) → detect ok? → register → enabled`;
//! detect fail without consent stops at `needs_install` (the plugin stays
//! unusable); with consent the declared installer runs, and any failure lands
//! on `failed`. The user is the only source of the install consent flag.

use crate::domain::directory::{CommandDecl, ProvisionSection};
use ark_common::{ArkError, Result};
use ark_contract::{McpServerEntry, McpTransportKind, PluginEnableRequest, PluginProvisionView};
use ark_ports::McpServerService;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Detect step budget: a tool that cannot answer within ten seconds counts as
/// absent, so the UI can offer installation immediately.
pub const DETECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Install step budget: package managers can be slow on cold caches.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
/// Detail text cap, so a chatty installer cannot bloat state or UI.
const DETAIL_CAP: usize = 2000;
/// State file next to `plugin.json`, inside the plugin's own directory.
const STATE_FILE: &str = "provision-state.json";

/// One plugin's provision declarations, resolved against its directory.
#[derive(Debug, Clone)]
pub struct ProvisionSpec {
    pub plugin_id: String,
    pub plugin_dir: PathBuf,
    pub section: ProvisionSection,
}

/// Persisted lifecycle state. Wire spellings match `PluginProvisionView.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionState {
    Disabled,
    NeedsInstall,
    Enabled,
    Failed,
}

impl ProvisionState {
    pub fn as_str(self) -> &'static str {
        match self {
            ProvisionState::Disabled => "disabled",
            ProvisionState::NeedsInstall => "needs_install",
            ProvisionState::Enabled => "enabled",
            ProvisionState::Failed => "failed",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "disabled" => ProvisionState::Disabled,
            "needs_install" => ProvisionState::NeedsInstall,
            "enabled" => ProvisionState::Enabled,
            "failed" => ProvisionState::Failed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredState {
    state: ProvisionState,
    detail: String,
}

impl Default for StoredState {
    fn default() -> Self {
        Self {
            state: ProvisionState::Disabled,
            detail: String::new(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct WireState {
    state: String,
    #[serde(default)]
    detail: String,
}

impl StoredState {
    fn load(plugin_dir: &Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(plugin_dir.join(STATE_FILE)) else {
            return Self::default();
        };
        match serde_json::from_str::<WireState>(&raw) {
            Ok(wire) => match ProvisionState::parse(&wire.state) {
                Some(state) => Self {
                    state,
                    detail: wire.detail,
                },
                None => Self::default(),
            },
            Err(_) => Self::default(),
        }
    }

    fn save(&self, plugin_dir: &Path) {
        let wire = WireState {
            state: self.state.as_str().to_string(),
            detail: self.detail.clone(),
        };
        if let Ok(json) = serde_json::to_string(&wire) {
            let _ = std::fs::write(plugin_dir.join(STATE_FILE), json);
        }
    }
}

/// Outcome of one governed command run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub success: bool,
    pub detail: String,
}

/// The only way the host executes a provision command. Test seam for the flow.
pub trait CommandRunner: Send + Sync {
    fn run(&self, command: &str, args: &[String], timeout: Duration) -> RunOutcome;
}

fn drain(mut pipe: impl Read, cap: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                // Keep draining past the cap so the pipe never blocks the child;
                // only the first `cap` bytes are retained.
                if buf.len() < cap {
                    let end = (buf.len() + n).min(cap);
                    buf.extend_from_slice(&chunk[..end - buf.len()]);
                }
            }
        }
    }
    buf
}

fn cap_text(text: String) -> String {
    text.chars().take(DETAIL_CAP).collect()
}

/// Production runner: bare binary resolved through `PATH`, no shell, cleared
/// environment, output drained on helper threads (a full pipe would deadlock
/// the wait loop), killed at the deadline.
pub struct OsCommandRunner;

impl CommandRunner for OsCommandRunner {
    fn run(&self, command: &str, args: &[String], timeout: Duration) -> RunOutcome {
        let mut child = match Command::new(command)
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", std::env::var("HOME").unwrap_or_default())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                return RunOutcome {
                    success: false,
                    detail: cap_text(format!("spawn {command}: {err}")),
                }
            }
        };
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let stdout_thread = thread::spawn(move || {
            stdout_pipe
                .map(|pipe| drain(pipe, DETAIL_CAP))
                .unwrap_or_default()
        });
        let stderr_thread = thread::spawn(move || {
            stderr_pipe
                .map(|pipe| drain(pipe, DETAIL_CAP))
                .unwrap_or_default()
        });

        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                Ok(None) => thread::sleep(Duration::from_millis(100)),
                Err(err) => {
                    return RunOutcome {
                        success: false,
                        detail: cap_text(format!("wait {command}: {err}")),
                    }
                }
            }
        };
        let stdout = stdout_thread.join().unwrap_or_default();
        let stderr = stderr_thread.join().unwrap_or_default();

        match status {
            None => RunOutcome {
                success: false,
                detail: cap_text(format!("{command} timed out after {timeout:?}")),
            },
            Some(status) if status.success() => RunOutcome {
                success: true,
                detail: cap_text(String::from_utf8_lossy(&stdout).trim().to_string()),
            },
            Some(status) => {
                let mut detail = String::from_utf8_lossy(&stderr).trim().to_string();
                if detail.is_empty() {
                    detail = String::from_utf8_lossy(&stdout).trim().to_string();
                }
                RunOutcome {
                    success: false,
                    detail: cap_text(format!("{command} exited with {status}: {detail}")),
                }
            }
        }
    }
}

/// The enable flow for every plugin that declares a `provision` section.
/// Host-owned: a plugin can never reach this struct.
pub struct ProvisionFlow {
    specs: BTreeMap<String, ProvisionSpec>,
    states: Mutex<BTreeMap<String, StoredState>>,
    runner: Arc<dyn CommandRunner>,
    /// Attached by the composition root after the MCP registry exists.
    /// `OnceLock`: set exactly once, never mutated afterwards.
    mcp_servers: OnceLock<Arc<dyn McpServerService>>,
}

impl ProvisionFlow {
    /// Build the flow from load-time specs, seeding persisted state. An
    /// `enabled` entry re-runs detection so a vanished binary downgrades to
    /// `needs_install` instead of a stale success.
    pub fn load(specs: Vec<ProvisionSpec>, runner: Arc<dyn CommandRunner>) -> Self {
        let mut states = BTreeMap::new();
        for spec in &specs {
            let mut state = StoredState::load(&spec.plugin_dir);
            if state.state == ProvisionState::Enabled {
                let outcome = self_detect(&*runner, &spec.section.detect);
                if !outcome.success {
                    state = StoredState {
                        state: ProvisionState::NeedsInstall,
                        detail: format!("binary missing at boot: {}", outcome.detail),
                    };
                    state.save(&spec.plugin_dir);
                }
            }
            states.insert(spec.plugin_id.clone(), state);
        }
        Self {
            specs: specs
                .into_iter()
                .map(|spec| (spec.plugin_id.clone(), spec))
                .collect(),
            states: Mutex::new(states),
            runner,
            mcp_servers: OnceLock::new(),
        }
    }

    /// Attach the registry the declared MCP server is registered into.
    pub fn attach_mcp_servers(&self, service: Arc<dyn McpServerService>) {
        let _ = self.mcp_servers.set(service);
    }

    /// Wire view of one plugin's provision state, or `None` when the plugin
    /// declares no section.
    pub fn status(&self, plugin_id: &str) -> Option<PluginProvisionView> {
        let spec = self.specs.get(plugin_id)?;
        let state = self
            .lock_states()
            .get(plugin_id)
            .cloned()
            .unwrap_or_default();
        Some(self.view(spec, state))
    }

    /// Run the enable flow end to end. Every step is host-governed; the only
    /// input from outside is the user's install consent flag.
    pub async fn enable(
        &self,
        plugin_id: &str,
        request: PluginEnableRequest,
    ) -> Result<PluginProvisionView> {
        let spec = self.specs.get(plugin_id).ok_or_else(|| {
            ArkError::validation(format!("plugin {plugin_id} declares no provision section"))
        })?;

        // 1) Presence check.
        let detect = self
            .run_blocking(&spec.section.detect, DETECT_TIMEOUT)
            .await;
        if detect.success {
            return self.register(spec).await;
        }

        // 2) Missing tool without consent: the plugin stays unusable.
        if !request.install {
            let state = StoredState {
                state: ProvisionState::NeedsInstall,
                detail: detect.detail,
            };
            self.persist(spec, &state);
            return Ok(self.view(spec, state));
        }

        // 3) Install behind the user's consent.
        let install = self
            .run_blocking(&spec.section.install, INSTALL_TIMEOUT)
            .await;
        if !install.success {
            let state = StoredState {
                state: ProvisionState::Failed,
                detail: format!("install failed: {}", install.detail),
            };
            self.persist(spec, &state);
            return Ok(self.view(spec, state));
        }

        // 4) Re-detect after installation.
        let detect = self
            .run_blocking(&spec.section.detect, DETECT_TIMEOUT)
            .await;
        if !detect.success {
            let state = StoredState {
                state: ProvisionState::Failed,
                detail: format!("installed but detect failed: {}", detect.detail),
            };
            self.persist(spec, &state);
            return Ok(self.view(spec, state));
        }

        // 5) Register the declared MCP server.
        self.register(spec).await
    }

    /// Upsert the declared external MCP server; success enables the plugin.
    async fn register(&self, spec: &ProvisionSpec) -> Result<PluginProvisionView> {
        let Some(service) = self.mcp_servers.get() else {
            let state = StoredState {
                state: ProvisionState::Failed,
                detail: "mcp server registry is not attached in this build".to_string(),
            };
            self.persist(spec, &state);
            return Ok(self.view(spec, state));
        };
        let entry = McpServerEntry {
            id: spec.section.mcp_server.id.clone(),
            transport: McpTransportKind::Stdio,
            enabled: true,
            command: Some(spec.section.mcp_server.command.clone()),
            args: spec.section.mcp_server.args.clone(),
            env: Default::default(),
            url: None,
            headers: Default::default(),
            allow_tools: spec.section.mcp_server.allow_tools.clone(),
        };
        match service.upsert(entry).await {
            Ok(_) => {
                let state = StoredState {
                    state: ProvisionState::Enabled,
                    detail: String::new(),
                };
                self.persist(spec, &state);
                Ok(self.view(spec, state))
            }
            Err(err) => {
                let state = StoredState {
                    state: ProvisionState::Failed,
                    detail: format!("mcp registration failed: {}", err.message),
                };
                self.persist(spec, &state);
                Ok(self.view(spec, state))
            }
        }
    }

    /// Blocking subprocess work stays off the async workers.
    async fn run_blocking(&self, cmd: &CommandDecl, timeout: Duration) -> RunOutcome {
        let runner = Arc::clone(&self.runner);
        let command = cmd.command.clone();
        let args = cmd.args.clone();
        tokio::task::spawn_blocking(move || runner.run(&command, &args, timeout))
            .await
            .unwrap_or_else(|err| RunOutcome {
                success: false,
                detail: cap_text(format!("runner join: {err}")),
            })
    }

    fn persist(&self, spec: &ProvisionSpec, state: &StoredState) {
        self.lock_states()
            .insert(spec.plugin_id.clone(), state.clone());
        state.save(&spec.plugin_dir);
    }

    fn view(&self, spec: &ProvisionSpec, state: StoredState) -> PluginProvisionView {
        PluginProvisionView {
            plugin_id: spec.plugin_id.clone(),
            state: state.state.as_str().to_string(),
            detail: state.detail,
            install_command: format!(
                "{} {}",
                spec.section.install.command,
                spec.section.install.args.join(" ")
            ),
            mcp_server_id: spec.section.mcp_server.id.clone(),
        }
    }

    fn lock_states(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, StoredState>> {
        self.states.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn self_detect(runner: &dyn CommandRunner, detect: &CommandDecl) -> RunOutcome {
    runner.run(&detect.command, &detect.args, DETECT_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::{McpServerListResponse, McpServerTestResponse};
    use async_trait::async_trait;
    use std::collections::VecDeque;

    /// Detect results pop per run so one runner can script detect-fail then
    /// detect-ok around a successful install.
    struct FakeRunner {
        detect_results: Mutex<VecDeque<bool>>,
        install_ok: bool,
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, command: &str, _args: &[String], _timeout: Duration) -> RunOutcome {
            if command == "npm" {
                return RunOutcome {
                    success: self.install_ok,
                    detail: if self.install_ok {
                        String::new()
                    } else {
                        "network unreachable".to_string()
                    },
                };
            }
            let ok = self
                .detect_results
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front()
                .unwrap_or(true);
            RunOutcome {
                success: ok,
                detail: if ok {
                    String::new()
                } else {
                    "detect-cmd not found".to_string()
                },
            }
        }
    }

    struct FakeMcp {
        upserts: Mutex<Vec<McpServerEntry>>,
    }

    #[async_trait]
    impl McpServerService for FakeMcp {
        fn list(&self) -> McpServerListResponse {
            McpServerListResponse {
                servers: Vec::new(),
            }
        }

        async fn upsert(&self, entry: McpServerEntry) -> Result<McpServerListResponse> {
            self.upserts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(entry);
            Ok(McpServerListResponse {
                servers: Vec::new(),
            })
        }

        async fn delete(&self, _id: &str) -> Result<McpServerListResponse> {
            Ok(McpServerListResponse {
                servers: Vec::new(),
            })
        }

        async fn test(&self, _id: &str) -> McpServerTestResponse {
            McpServerTestResponse::default()
        }

        async fn connect_all(&self) -> Vec<Arc<dyn ark_ports::Tool>> {
            Vec::new()
        }
    }

    fn spec(dir: &Path) -> ProvisionSpec {
        ProvisionSpec {
            plugin_id: "ark.plugins.test".to_string(),
            plugin_dir: dir.to_path_buf(),
            section: ProvisionSection {
                detect: CommandDecl {
                    command: "detect-cmd".to_string(),
                    args: vec!["status".to_string()],
                },
                install: CommandDecl {
                    command: "npm".to_string(),
                    args: vec![
                        "install".to_string(),
                        "-g".to_string(),
                        "agent-desktop".to_string(),
                    ],
                },
                mcp_server: crate::domain::directory::McpServerProvision {
                    id: "test-tool".to_string(),
                    command: "detect-cmd".to_string(),
                    args: vec!["mcp".to_string()],
                    allow_tools: vec![],
                },
            },
        }
    }

    fn flow(
        dir: &Path,
        detect_results: Vec<bool>,
        install_ok: bool,
        mcp: Arc<FakeMcp>,
    ) -> ProvisionFlow {
        let flow = ProvisionFlow::load(
            vec![spec(dir)],
            Arc::new(FakeRunner {
                detect_results: Mutex::new(detect_results.into()),
                install_ok,
            }),
        );
        flow.attach_mcp_servers(mcp as Arc<dyn McpServerService>);
        flow
    }

    #[tokio::test]
    async fn detect_ok_registers_mcp_without_install() {
        let dir = tempfile::tempdir().unwrap();
        let mcp = Arc::new(FakeMcp {
            upserts: Mutex::new(Vec::new()),
        });
        let flow = flow(dir.path(), vec![true], false, Arc::clone(&mcp));

        let view = flow
            .enable("ark.plugins.test", PluginEnableRequest { install: false })
            .await
            .unwrap();

        assert_eq!(view.state, "enabled");
        assert_eq!(view.mcp_server_id, "test-tool");
        let upserts = mcp.upserts.lock().unwrap();
        assert_eq!(upserts.len(), 1);
        assert_eq!(upserts[0].id, "test-tool");
        assert_eq!(upserts[0].transport, McpTransportKind::Stdio);
        assert_eq!(upserts[0].command.as_deref(), Some("detect-cmd"));
    }

    #[tokio::test]
    async fn detect_fail_without_consent_stays_unusable() {
        let dir = tempfile::tempdir().unwrap();
        let mcp = Arc::new(FakeMcp {
            upserts: Mutex::new(Vec::new()),
        });
        let flow = flow(dir.path(), vec![false], false, Arc::clone(&mcp));

        let view = flow
            .enable("ark.plugins.test", PluginEnableRequest { install: false })
            .await
            .unwrap();

        assert_eq!(view.state, "needs_install");
        assert!(view
            .install_command
            .contains("npm install -g agent-desktop"));
        assert!(mcp.upserts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn consent_installs_then_enables() {
        let dir = tempfile::tempdir().unwrap();
        let mcp = Arc::new(FakeMcp {
            upserts: Mutex::new(Vec::new()),
        });
        // detect fails, install succeeds, re-detect succeeds.
        let flow = flow(dir.path(), vec![false, true], true, Arc::clone(&mcp));

        let view = flow
            .enable("ark.plugins.test", PluginEnableRequest { install: true })
            .await
            .unwrap();

        assert_eq!(view.state, "enabled");
        assert_eq!(mcp.upserts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn install_failure_leaves_the_plugin_unusable() {
        let dir = tempfile::tempdir().unwrap();
        let mcp = Arc::new(FakeMcp {
            upserts: Mutex::new(Vec::new()),
        });
        let flow = flow(dir.path(), vec![false, false], false, Arc::clone(&mcp));

        let view = flow
            .enable("ark.plugins.test", PluginEnableRequest { install: true })
            .await
            .unwrap();

        assert_eq!(view.state, "failed");
        assert!(view.detail.contains("install failed"));
        assert!(mcp.upserts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn state_persists_across_reload_and_boot_detect_downgrades() {
        let dir = tempfile::tempdir().unwrap();
        let mcp = Arc::new(FakeMcp {
            upserts: Mutex::new(Vec::new()),
        });
        let boot = flow(dir.path(), vec![true], false, Arc::clone(&mcp));
        let view = boot
            .enable("ark.plugins.test", PluginEnableRequest { install: false })
            .await
            .unwrap();
        assert_eq!(view.state, "enabled");

        // Reload with detection still passing: state survives.
        let reloaded = flow(dir.path(), vec![true, true], false, Arc::clone(&mcp));
        assert_eq!(
            reloaded.status("ark.plugins.test").unwrap().state,
            "enabled"
        );

        // Reload with the binary gone: boot detect downgrades to needs_install.
        let degraded = flow(
            dir.path(),
            vec![false, false, false],
            false,
            Arc::clone(&mcp),
        );
        assert_eq!(
            degraded.status("ark.plugins.test").unwrap().state,
            "needs_install"
        );
    }

    #[test]
    fn unknown_plugin_id_is_a_validation_error() {
        let dir = tempfile::tempdir().unwrap();
        let mcp = Arc::new(FakeMcp {
            upserts: Mutex::new(Vec::new()),
        });
        let flow = flow(dir.path(), vec![true], false, Arc::clone(&mcp));
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(flow.enable("other.plugin", PluginEnableRequest { install: false }))
            .unwrap_err();
        assert!(err.message.contains("declares no provision section"));
    }

    #[test]
    fn section_validation_rejects_paths_and_unknown_installers() {
        let mut section = spec(Path::new("/tmp")).section;
        section.detect.command = "/usr/bin/agent-desktop".to_string();
        assert!(section.validate().is_err());

        section = spec(Path::new("/tmp")).section;
        section.install.command = "curl".to_string();
        assert!(section.validate().is_err());

        section = spec(Path::new("/tmp")).section;
        section.mcp_server.id = "has space".to_string();
        assert!(section.validate().is_err());

        section = spec(Path::new("/tmp")).section;
        assert!(section.validate().is_ok());
    }
}
