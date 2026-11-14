//! Explicit composition root (design §5.3).
//!
//! Every concrete implementation is named here and nowhere else. The rule the
//! design asks for — no `if plugin_id == ...` — is enforced by shape: each
//! collaborator is a field of [`Wiring`], injected into the pieces that need it,
//! and nothing downstream can reach for a different implementation.

//! # Layout
//!
//! The values exported here ([`SYSTEM_PROMPT`], [`CHUNK_BUFFER`], [`Wiring`],
//! [`build`]) stay in this file; the assembly steps live in sibling modules:
//! [`tools`] builds the registries, [`turns`] drives one parent turn,
//! [`history`] rebuilds the model-visible prefix.

use std::collections::HashSet;
use std::sync::Arc;

use ark_agent::EventSinkWriter;
use ark_agent::ToolRunner as AgentToolRunner;
use ark_agent::{
    CancelToken, CompletionInbox, GateBroker, ModelSeed, Reactor, ReactorConfig, SteerQueue,
    ToolCallRequest, TurnEvents, TurnLimits, TurnMachine, TurnOutcome,
};
use ark_common::{Result, ThreadId, TurnId};
use ark_config::{ConfigStore, RuntimeConfig};
use ark_obs::UsageRecorder;
use ark_ports::{
    BaseServices, EventSink, ModelClient, ModelMessage, ModelRole, SandboxPolicy, SessionStore,
    ToolCallRef, ToolOutput, ToolRegistryPort,
};
use ark_store::{AttachmentStore, FileSessionStore, StoreConfig, DEFAULT_MAX_ATTACHMENT_BYTES};
use ark_tool::{ApprovalPolicy, Autonomy, BuiltinSandboxPolicy, RegistryToolRunner, ToolRegistry};

use ark_server::{AppState, TurnEnd, TurnJob, TurnRunner, TurnSource};

use crate::base::{
    McpServerAdapter, MemoryAdapter, ProviderAdapter, SkillAdapter, WorkspaceAdapter,
};
use crate::cli::ServeArgs;
use crate::load_skill::LoadSkillTool;
use crate::model;
use crate::paths::RuntimePaths;
use crate::workspace::WorkspaceRoots;

mod history;
mod tools;
pub(crate) mod turn_common;
mod turns;

use tools::{build_child_tools, build_tools};
use turns::{abort_interrupted_turns, AgentTurnRunner};
#[cfg(test)]
mod tests;

/// System prefix. Static text only, so the prompt-cache prefix stays byte-stable.
pub const SYSTEM_PROMPT: &str = "\
You are ark, a coding agent inside the smart-ark desktop application.
Work in small, verifiable steps. Prefer reading the repository over guessing.
Every tool call is logged; never claim a check you did not run.
Respond in the user's language. Unless the user writes in another language, \
write user-visible content (answers, explanations) and all reasoning/thinking \
text in Simplified Chinese.";

/// Default model-delta channel capacity for one reactor round.
pub const CHUNK_BUFFER: usize = 64;

/// Everything the process owns, constructed exactly once.
pub struct Wiring {
    pub state: AppState,
    pub store: Arc<FileSessionStore>,
    pub tools: Arc<ToolRegistry>,
    pub config: Arc<ConfigStore>,
    pub usage: Arc<UsageRecorder>,
    pub model_label: String,
    /// Scheduled-task orchestration. Injected here so `serve` can start the
    /// loop; `run` holds it without starting anything.
    pub schedules: Arc<ark_server::SchedulerService>,
    /// IM-channel orchestration. Same lifecycle as `schedules`: connections
    /// only run under `serve`, never in a one-shot headless `run`.
    pub im: Arc<ark_server::ImService>,
    /// The local plugin host, kept alive for the process lifetime.
    ///
    /// Dropping it would run every plugin's `Registration::drop`, silently
    /// unregistering the pages, panels and tools the runtime just published.
    pub plugins: crate::plugin_host::PluginRuntime,
}

/// Load `config/config.json` for startup, keeping the three outcomes apart.
///
/// A missing file is a normal first run (defaults, no log line); an unreadable or
/// unparseable file is an error the caller turns into a boot failure; a file that
/// only partially matches the schema loads as before, but `load_or_default`
/// names every defaulted or ignored key in a warning, so a GUI-shaped document
/// can no longer downgrade `auto_continue`/`max_concurrency` silently.
fn load_startup_config(config_dir: &std::path::Path) -> std::result::Result<RuntimeConfig, String> {
    ark_config::load_or_default(&ark_config::config_path(config_dir))
}

/// Build the runtime for `args`.
pub async fn build(args: &ServeArgs) -> Result<Wiring> {
    let paths = RuntimePaths::from_data_dir(&args.data_dir);
    paths.ensure()?;
    let config_path = ark_config::config_path(&paths.config);
    let loaded = load_startup_config(&paths.config).map_err(|message| {
        ark_common::ArkError::validation(format!(
            "cannot load {}: {message}",
            config_path.display()
        ))
    })?;
    let runtime_config = RuntimeConfig {
        port: args.port,
        data_dir: paths.data.clone(),
        ..loaded
    };
    let config = Arc::new(ConfigStore::new(runtime_config));

    // Real store: the session directory is the system of record.
    let store = Arc::new(FileSessionStore::open(StoreConfig::new(&paths.data))?);
    let attachments: Arc<dyn ark_ports::AttachmentStore> = Arc::new(AttachmentStore::open(
        &paths.data,
        DEFAULT_MAX_ATTACHMENT_BYTES,
    )?);
    // Scheduled tasks live in their own document (`schedules.json`), not in the
    // config file: the runtime rewrites them on every run, and a settings file
    // that changes that often would fight the config store's versioning.
    let schedules = Arc::new(ark_server::SchedulerService::new(Arc::new(
        ark_store::ScheduleFileStore::open(&paths.tasks)?,
    )));
    // IM channels live in their own document (`im.json`), same reasoning as
    // `schedules.json`. The transport client reuses the governed model/HTTP
    // policy (proxy/TLS settings stay process-wide); long polling needs no
    // client-level timeout, which `None` expresses.
    let im_client =
        ark_http::transport_client(&ark_http::HttpPolicy::default(), "ark-im/channel", None)
            .unwrap_or_else(|err| {
                tracing::warn!(
                    error = %err.message,
                    "falling back to default http transport for im channels"
                );
                reqwest::Client::new()
            });
    let im = ark_server::ImService::new(
        Arc::new(ark_store::ImFileStore::open(&paths.im)?),
        im_client.clone(),
    );
    // WeChat QR login is an optional capability wired by the composition root;
    // it needs a governed client of its own (same policy).
    im.set_login_flow(Arc::new(ark_im::WeixinLoginFlow::new(im_client)));
    store.reconcile()?;
    let interrupted = abort_interrupted_turns(store.as_ref()).await?;
    if interrupted != 0 {
        tracing::warn!(
            turns = interrupted,
            "aborted parent turns left open by a previous runtime"
        );
    }
    let store_port: Arc<dyn SessionStore> = store.clone();
    let sink: Arc<dyn EventSink> = store.clone();

    // The sandbox policy is shared by parent and child turns: a delegated task
    // must never get a laxer sandbox than the turn that spawned it.
    let policy: Arc<dyn SandboxPolicy> = Arc::new(BuiltinSandboxPolicy::new(
        autonomy_from_override(Some(&config.snapshot().default_autonomy))?,
    ));
    // A workspace root is leased once per turn. Switching is handled through
    // the workspace HTTP surface and never re-reads the Hot config mid-turn.
    let roots = Arc::new(WorkspaceRoots::new(
        args.workspace.clone(),
        &paths.workspace,
    )?);
    let usage = Arc::new(UsageRecorder::new());

    // ---- platform services (design §7.5.4) ----
    //
    // Built here and only here. Each service receives its categorized runtime
    // directory, and the HTTP layer sees it through the `ark-ports` traits below.
    let providers = Arc::new(ark_provider::ProviderStore::with_catalog_client(
        paths.config.clone(),
        ark_llm::ModelCatalogClient::new(crate::model::governed_transport()),
    ));
    // Skills config is a `Hot` field: the runtime reads it through this source
    // on every scan/resolve, so a saved settings patch takes effect on the next
    // rescan without a restart. The workspace root is still captured once —
    // switching workspaces goes through its own HTTP surface and leases a new
    // root per turn.
    let skills = Arc::new(ark_skills::SkillRuntime::new(
        skills_config_source(config.clone(), paths.skills.clone()),
        roots.current(),
    ));
    let workspace = Arc::new(ark_workspace::WorkspaceInspector::new());
    let mcp = Arc::new(ark_mcp::McpServerRegistry::new(paths.mcp.clone()));
    let memory = Arc::new(ark_store::MemoryFileStore::open(&paths.data)?);

    // ---- UI catalog (design §7.5.3) ----
    //
    // The Code screen is registered through the same bus any plugin uses, so
    // `GET /v1/runtime/capabilities` and the renderer navigation have exactly
    // one source. The bus derives `ui_page`/`ui_panel` from these registrations
    // and the route composes `tool`/`model_provider` from their own ports, so
    // the composition root keeps no parallel capability list.
    let ui_bus = Arc::new(ark_ui::UiBus::new());
    let code_page_id = ui_bus.register_page(ark_ui::CODE_UI_PLUGIN_ID, ark_ui::code_page())?;
    // The handle must outlive assembly: dropping it would unregister the page.
    code_page_id.forget();
    for (slot, panel) in ark_ui::code_panels() {
        ui_bus
            .register_panel(ark_ui::CODE_UI_PLUGIN_ID, slot, panel)?
            .forget();
    }
    tracing::info!("registered built-in UI catalog");

    // ---- local plugins (design §7.7) ----
    //
    // One MCP bus and one governed HTTP egress serve both plugins and external
    // MCP servers. The host registers plugin tools on the bus while loading, and
    // the tool group below merges them with the external adapters, so there is
    // still exactly one catalog.
    //
    // The runtime must outlive assembly: dropping it runs every `Registration`'s
    // `Drop`, which would unregister the pages, panels and tools it published.
    //
    // Plugins get no `EventSink`: the platform handle exposes the raw sink, so
    // handing it over would let a plugin append arbitrary events to any thread.
    // Nothing in this version needs it, so the field stays `None`.
    let plugin_mcp = Arc::new(ark_mcp::McpBus::default());
    let plugins = crate::plugin_host::assemble(&paths.data, &ui_bus, Arc::clone(&plugin_mcp))?;

    // The runtime's own `web_fetch` tool shares the plugins' governed egress
    // under its `ark.builtin.web` identity. The unrestricted allowlist is only
    // constructible here (never through a plugin manifest); the per-request
    // SSRF guard lives in the tool, and HTTPS-only plus size/timeout caps come
    // from the shared HttpPolicy.
    plugins.http().register_grant(
        "ark.builtin.web",
        env!("CARGO_PKG_VERSION"),
        ark_http::HostAllowlist::allow_all(),
    )?;

    // The provider table is the source of truth for the model client, so a
    // provider added through the GUI takes effect on the next turn instead of
    // requiring a restart. `model_providers` in config.json stays honoured as a
    // legacy fallback for a hand-written config.
    let provider_endpoint = providers.resolve(None);
    let (model, model_label) = match provider_endpoint {
        Some(endpoint) => model::select_from_config(endpoint, Arc::clone(&usage)),
        None => model::select(&config.snapshot(), Arc::clone(&usage)),
    };

    // The hook bus (requirements/plugin-hook-bus.md, P1). The composition root
    // installs exactly one broker for the whole process; until a plugin supplies
    // authorized hooks this is the no-op implementation, which is what keeps the
    // hook decorators byte-for-byte transparent. The real multiplexer (P2+) is
    // built here from the plugin host's authorizations — never per route.
    let hooks: Arc<dyn ark_ports::HookBroker> = Arc::new(ark_ports::NoHooks);

    // ---- sub-agent surface (design §7.8) ----
    //
    // Construction order is deliberate:
    //
    //   1. the **child** registry (builtins only, no `delegate_task`), which is
    //      what makes `max_depth = 1` structural rather than a runtime check;
    //   2. the executor that drives a child turn with the same reactor;
    //   3. the delegation runtime, which owns budget, records and handles;
    //   4. the **parent** registry, which adds `delegate_task` on top.
    //
    // Building the parent registry last is the only way to avoid a cycle: the
    // tool needs the runner, and the runner needs an executor that needs tools.
    let child_tools = Arc::new(build_child_tools(
        Arc::clone(&skills),
        Arc::clone(plugins.http()) as Arc<dyn ark_ports::HttpCapability>,
    )?);
    let subagent_policy = Arc::new(crate::child::ConfigSubagentPolicy::new(Arc::clone(&config)));

    let child_executor = Arc::new(crate::child::ChildTurnExecutorImpl::new(
        Arc::clone(&config),
        Arc::clone(&providers),
        Arc::clone(&usage),
        Arc::clone(&model),
        Arc::clone(&child_tools),
        Arc::clone(&sink),
        Arc::clone(&store_port),
        Arc::clone(&policy),
        Arc::clone(&roots),
        Arc::clone(&skills),
        Arc::clone(&hooks),
    ));
    let delegation = Arc::new(ark_delegation::DelegationRuntime::new(
        Arc::clone(&store_port),
        Arc::clone(&sink),
        Arc::clone(&subagent_policy) as Arc<dyn ark_ports::SubagentPolicySource>,
        child_executor,
    ));
    // Subscribe before hydrate so repaired terminal notifications cannot race
    // past the auto-continuation listener constructed below.
    let completion_notifications = delegation.subscribe_completions();
    // Load the authoritative `children/runs.jsonl` records before the HTTP
    // surface exists. Without this the in-memory registry starts empty, so every
    // historical child would answer 404 after a restart even though its record
    // and session are on disk (design §7.4.1). It also converges runs that were
    // still queued/running when the previous process died.
    let hydrated = delegation.hydrate().await?;
    tracing::info!(children = hydrated, "loaded child-run records");

    let tools = Arc::new(build_tools(
        Arc::clone(&delegation) as Arc<dyn ark_ports::SubagentRunner>,
        Arc::clone(&subagent_policy) as Arc<dyn ark_ports::SubagentPolicySource>,
        Arc::clone(&skills),
        Arc::clone(plugins.http()) as Arc<dyn ark_ports::HttpCapability>,
        Arc::clone(&store_port),
    )?);
    // One `mcp` group: external server adapters plus local plugin tools. Both
    // families are qualified `<provider>.<tool>`, so a collision is a real
    // configuration conflict and `replace_group` refuses it rather than letting
    // one family silently shadow the other.
    let mut mcp_tools = mcp.connect_all().await;
    mcp_tools.extend(plugins.tools());
    tools.replace_group("mcp", mcp_tools)?;
    let tool_runner = Arc::new(RegistryToolRunner::new(
        Arc::clone(&tools),
        Arc::clone(&policy),
        roots.current(),
    ));

    // The HTTP gate handlers and every parent reactor receive this exact same
    // rendezvous. A request waits until the Code UI resolves it; `AllowAllGates`
    // would make approval cards cosmetic. The reactor's copy is wrapped in an
    // auto-deny decorator (read from the live config): an unattended turn fails
    // fast with a durable refusal instead of parking on a gate nobody can
    // answer (requirements/approval-rules.md §4). AppState keeps the bare
    // rendezvous because its HTTP handlers resolve by id and never wait.
    let interactive = Arc::new(ark_server::InteractiveGates::new());
    let reactor_gates = Arc::new(ark_agent::AutoDenyGates::new(
        interactive.clone(),
        ark_tool::unattended_forwards(&config.snapshot().approvals),
    ));
    let gate_broker: Arc<dyn GateBroker> = reactor_gates;

    let runner = Arc::new(AgentTurnRunner {
        providers: Arc::clone(&providers),
        config: Arc::clone(&config),
        usage: Arc::clone(&usage),
        tools: tool_runner,
        store: Arc::clone(&store_port),
        sink,
        roots: Arc::clone(&roots),
        tool_registry: Arc::clone(&tools),
        skills: Arc::clone(&skills),
        memory: Arc::clone(&memory),
        attachments: Arc::clone(&attachments),
        gates: gate_broker,
        hooks: Arc::clone(&hooks),
    });

    let mcp_adapter = Arc::new(McpServerAdapter::new(
        Arc::clone(&mcp),
        Arc::clone(&tools),
        {
            let plugin_mcp = Arc::clone(&plugin_mcp);
            Arc::new(move || crate::plugin_host::plugin_tools(&plugin_mcp))
        },
    ));
    // The provision enable flow registers its declared external MCP server
    // through the same adapter the GUI edits (requirements/plugin-provisioning.md).
    plugins
        .host()
        .attach_mcp_servers(Arc::clone(&mcp_adapter) as Arc<dyn ark_ports::McpServerService>);

    let base = BaseServices::new()
        .with_providers(Arc::new(ProviderAdapter::new(Arc::clone(&providers))))
        .with_title_model(Arc::new(crate::base::TitleModelAdapter::new(
            Arc::clone(&providers),
            Arc::clone(&usage),
        )))
        .with_skills(Arc::new(SkillAdapter::new(Arc::clone(&skills))))
        .with_workspace(Arc::new(WorkspaceAdapter::new(
            Arc::clone(&workspace),
            Arc::clone(&roots),
        )))
        .with_mcp(Arc::clone(&mcp_adapter) as Arc<dyn ark_ports::McpServerService>)
        .with_memory(Arc::new(MemoryAdapter::new(Arc::clone(&memory))))
        .with_ui(Arc::clone(&ui_bus) as Arc<dyn ark_ports::UiCapability>)
        .with_plugins(Arc::clone(plugins.host()) as Arc<dyn ark_ports::PluginHostService>)
        .with_folder_dialog(Arc::new(crate::base::NativeFolderDialog));

    let state = AppState::new(
        store_port,
        tools.port(),
        Arc::clone(&config),
        Arc::clone(&usage),
        runner,
        paths.data.clone(),
        env!("CARGO_PKG_VERSION"),
    )
    .with_config_dir(paths.config.clone())
    .with_attachments(attachments)
    .with_base_services(base)
    .with_subagents(
        Arc::clone(&delegation) as Arc<dyn ark_ports::SubagentRunner>,
        Arc::clone(&delegation) as Arc<dyn ark_ports::SubagentRegistry>,
    )
    .with_gates(interactive)
    .with_schedules(Arc::clone(&schedules))
    .with_im(Arc::clone(&im))
    .with_auth_token(args.auth_token.clone())
    .with_hooks(hooks);

    spawn_completion_listener(state.clone(), completion_notifications);
    // Recover unacknowledged completions that were durable before this process
    // started (for example, a crash after `child_completed` but before debounce).
    ark_server::service::sync_auto_continue_policy(state.clone()).await;
    // The loop is NOT started here: a one-shot headless `run` reuses this
    // assembly and must not fire due schedules (or spend model budget) while
    // executing a single turn. `serve` calls `Wiring::schedules.spawn(...)`
    // after binding; `run` holds the handle without starting anything.

    Ok(Wiring {
        state,
        store,
        tools,
        config,
        usage,
        model_label,
        schedules,
        im,
        plugins,
    })
}

/// Forward durable child-completion wakeups to the per-thread debounce.
fn spawn_completion_listener(
    state: AppState,
    mut notifications: tokio::sync::broadcast::Receiver<ThreadId>,
) {
    tokio::spawn(async move {
        loop {
            match notifications.recv().await {
                Ok(thread_id) => {
                    ark_server::service::schedule_auto_continuation(state.clone(), thread_id).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Recover from the durable inbox through the same safe scan
                    // used at startup, including persisted interrupt suppression.
                    ark_server::service::sync_auto_continue_policy(state.clone()).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

pub(crate) fn autonomy_from_override(value: Option<&str>) -> ark_common::Result<Autonomy> {
    // The enum owns the vocabulary; this wrapper only supplies the "missing"
    // case, which the settings layer never produces.
    value
        .map(Autonomy::parse)
        .unwrap_or_else(|| Err(ark_common::ArkError::validation("autonomy is missing")))
}

pub(crate) fn autonomy_from_override_or_default(
    value: Option<&str>,
    config: &RuntimeConfig,
) -> ark_common::Result<Autonomy> {
    autonomy_from_override(value.or(Some(&config.default_autonomy)))
}

/// The live config source `SkillRuntime` re-reads on every scan/resolve.
///
/// Extracted from `build` so a test can assert that a saved skills patch is
/// visible through the same closure the composition root injects.
fn skills_config_source(
    config: Arc<ConfigStore>,
    global_skills: std::path::PathBuf,
) -> ark_skills::ConfigSource {
    Arc::new(move || {
        let settings = config.snapshot().skills.clone();
        // First root stays the app-owned `<root>/skills` directory (writes and
        // first-party skills live here); the shared `~/.agents/skills`
        // convention follows, then the user's configured extra dirs.
        let mut skill_roots = vec![global_skills.clone(), ark_skills::shared_skills_root()];
        skill_roots.extend(settings.extra_dirs.iter().map(std::path::PathBuf::from));
        ark_skills::SkillsConfig {
            enabled: settings.enabled,
            roots: skill_roots,
            ..ark_skills::SkillsConfig::default()
        }
    })
}

#[cfg(test)]
mod autonomy_tests {
    use super::*;

    #[test]
    fn unset_session_autonomy_uses_the_global_default() {
        let config = RuntimeConfig {
            default_autonomy: "strict".to_string(),
            ..RuntimeConfig::default()
        };
        assert_eq!(
            autonomy_from_override_or_default(None, &config).unwrap(),
            Autonomy::Strict
        );
    }
}
