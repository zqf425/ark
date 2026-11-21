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
///
/// The plan-maintenance sentence deliberately names only the `todo_write`
/// tool — the structured checkpoint line prefixes are the compactor's
/// internal protocol, not a model contract, and asking the model to emit
/// them makes those lines compete for the checkpoint's section quota
/// (review round 1, A2).
pub const SYSTEM_PROMPT: &str = "\
You are ark, a coding agent running as a standalone desktop tool.
Work in small, verifiable steps. Prefer reading the repository over guessing.
Every tool call is logged; never claim a check you did not run.
For long or multi-step tasks, create a plan with `todo_write` after the first \
exploration and update it as soon as a step completes or the direction changes.
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

/// Values the assembly steps hand to each other, in build order.
struct Assembly {
    config: Arc<ConfigStore>,
    store: Arc<FileSessionStore>,
    store_port: Arc<dyn SessionStore>,
    sink: Arc<dyn EventSink>,
    policy: Arc<dyn SandboxPolicy>,
    roots: Arc<WorkspaceRoots>,
    usage: Arc<UsageRecorder>,
}

/// Build the runtime for `args`.
///
/// Each assembly step is one private function below: the composition root
/// stays "explicit wiring only" while keeping every function inside the
/// AGENTS §4 length budget. Step order matters where a comment marks it —
/// e.g. the completion listener subscribes before hydration (§7.4.1).
pub async fn build(args: &ServeArgs, unattended: bool) -> Result<Wiring> {
    let paths = RuntimePaths::from_data_dir(&args.data_dir);
    paths.ensure()?;
    let ctx = prepare_runtime(args, &paths).await?;

    // ---- platform services (design §7.5.4) ----
    let platform = build_platform_services(&paths, &ctx.config, &ctx.roots)?;
    // ---- local plugins (design §7.7) ----
    let (plugins, plugin_mcp) = assemble_plugin_host(&paths, &platform.ui_bus)?;
    plugins.http().register_grant(
        "ark.builtin.web",
        env!("CARGO_PKG_VERSION"),
        ark_http::HostAllowlist::allow_all(),
    )?;
    let (model, model_label) =
        select_model(&platform.providers, &ctx.config, Arc::clone(&ctx.usage));
    let hooks: Arc<dyn ark_ports::HookBroker> = Arc::new(ark_ports::NoHooks);

    // ---- sub-agent surface (design §7.8) ----
    let agent = assemble_agent_surface(&ctx, &platform, &model, &hooks, &plugins).await?;

    let tool_runner =
        build_tool_runner(&platform, &agent, &plugins, &ctx.roots, &ctx.policy).await?;
    let (interactive, runner, mcp_adapter) = assemble_turn_pipeline(
        &ctx,
        &platform,
        &agent,
        &plugins,
        &hooks,
        tool_runner,
        &plugin_mcp,
        unattended,
    );
    let state = assemble_app_state(
        args,
        &paths,
        &ctx,
        &platform,
        &agent,
        &plugins,
        &interactive,
        &hooks,
        runner,
        &mcp_adapter,
    );

    spawn_completion_listener(state.clone(), agent.completion_notifications);
    // Recover unacknowledged completions that were durable before this process
    // started (for example, a crash after `child_completed` but before debounce).
    ark_server::sync_auto_continue_policy(state.clone()).await;
    // The loop is NOT started here: a one-shot headless `run` reuses this
    // assembly and must not fire due schedules (or spend model budget) while
    // executing a single turn. `serve` calls `Wiring::schedules.spawn(...)`
    // after binding; `run` holds the handle without starting anything.

    Ok(Wiring {
        state,
        store: Arc::clone(&ctx.store),
        tools: agent.tools,
        config: Arc::clone(&ctx.config),
        usage: Arc::clone(&ctx.usage),
        model_label,
        schedules: platform.schedules,
        im: platform.im,
        plugins,
    })
}

/// Open the store, recover turns left open by a previous runtime and build the
/// process-wide policy/workspace/usage singletons.
///
/// Recovery of half-open turns happens before any HTTP surface can observe
/// them: the session directory is the system of record.
async fn prepare_runtime(args: &ServeArgs, paths: &RuntimePaths) -> Result<Assembly> {
    let config = Arc::new(ConfigStore::new(load_runtime_config(paths, args)?));
    let store = Arc::new(FileSessionStore::open(StoreConfig::new(&paths.data))?);
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
    Ok(Assembly {
        config,
        store,
        store_port,
        sink,
        policy,
        roots,
        usage: Arc::new(UsageRecorder::new()),
    })
}

/// Build the gates, MCP adapter and turn runner that connect the agent surface
/// to the HTTP layer.
///
/// The HTTP gate handlers and every parent reactor receive this exact same
/// rendezvous. A request waits until the Code UI resolves it; `AllowAllGates`
/// would make approval cards cosmetic. The reactor's copy is wrapped in an
/// auto-deny decorator (read from the live config): an unattended turn fails
/// fast with a durable refusal instead of parking on a gate nobody can
/// answer (requirements/approval-rules.md §4). AppState keeps the bare
/// rendezvous because its HTTP handlers resolve by id and never wait.
#[allow(clippy::too_many_arguments)]
fn assemble_turn_pipeline(
    ctx: &Assembly,
    platform: &PlatformServices,
    agent: &AgentSurface,
    plugins: &crate::plugin_host::PluginRuntime,
    hooks: &Arc<dyn ark_ports::HookBroker>,
    tool_runner: Arc<RegistryToolRunner>,
    plugin_mcp: &Arc<ark_mcp::McpBus>,
    unattended: bool,
) -> (
    Arc<ark_server::InteractiveGates>,
    Arc<AgentTurnRunner>,
    Arc<McpServerAdapter>,
) {
    let interactive = Arc::new(ark_server::InteractiveGates::new());
    let gate_broker: Arc<dyn GateBroker> = interactive.clone();

    let mcp_adapter = Arc::new(McpServerAdapter::new(
        Arc::clone(&platform.mcp),
        Arc::clone(&agent.tools),
        {
            let plugin_mcp = Arc::clone(plugin_mcp);
            Arc::new(move || crate::plugin_host::plugin_tools(&plugin_mcp))
        },
    ));
    // The provision enable flow registers its declared external MCP server
    // through the same adapter the GUI edits (requirements/plugin-provisioning.md).
    plugins
        .host()
        .attach_mcp_servers(Arc::clone(&mcp_adapter) as Arc<dyn ark_ports::McpServerService>);

    let runner = Arc::new(AgentTurnRunner {
        providers: Arc::clone(&platform.providers),
        config: Arc::clone(&ctx.config),
        usage: Arc::clone(&ctx.usage),
        tools: tool_runner,
        store: Arc::clone(&ctx.store_port),
        sink: Arc::clone(&ctx.sink),
        roots: Arc::clone(&ctx.roots),
        tool_registry: Arc::clone(&agent.tools),
        skills: Arc::clone(&platform.skills),
        memory: Arc::clone(&platform.memory),
        attachments: Arc::clone(&platform.attachments),
        gates: gate_broker,
        unattended,
        hooks: Arc::clone(hooks),
    });
    (interactive, runner, mcp_adapter)
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

/// Merge the startup document with the CLI overrides into the effective config.
fn load_runtime_config(paths: &RuntimePaths, args: &ServeArgs) -> Result<RuntimeConfig> {
    let config_path = ark_config::config_path(&paths.config);
    let loaded = load_startup_config(&paths.config).map_err(|message| {
        ark_common::ArkError::validation(format!(
            "cannot load {}: {message}",
            config_path.display()
        ))
    })?;
    Ok(RuntimeConfig {
        port: args.port,
        data_dir: paths.data.clone(),
        ..loaded
    })
}

/// Everything built once for a capability area (design §7.5.4): schedules, IM,
/// attachments, providers, skills, workspace inspection, MCP, memory and the
/// UI/plugin surfaces the adapters need.
struct PlatformServices {
    schedules: Arc<ark_server::SchedulerService>,
    im: Arc<ark_server::ImService>,
    attachments: Arc<dyn ark_ports::AttachmentStore>,
    providers: Arc<ark_provider::ProviderStore>,
    skills: Arc<ark_skills::SkillRuntime>,
    workspace: Arc<ark_workspace::WorkspaceInspector>,
    mcp: Arc<ark_mcp::McpServerRegistry>,
    memory: Arc<ark_store::MemoryFileStore>,
    ui_bus: Arc<ark_ui::UiBus>,
}

/// Construct every platform service against its categorized runtime directory.
///
/// Scheduled tasks live in their own document (`schedules.json`), not in the
/// config file: the runtime rewrites them on every run, and a settings file
/// that changes that often would fight the config store's versioning. IM
/// channels are the same reasoning (`im.json`); the transport client reuses
/// the governed model/HTTP policy (proxy/TLS settings stay process-wide), and
/// long polling needs no client-level timeout, which `None` expresses.
fn build_platform_services(
    paths: &RuntimePaths,
    config: &Arc<ConfigStore>,
    roots: &Arc<WorkspaceRoots>,
) -> Result<PlatformServices> {
    let schedules = Arc::new(ark_server::SchedulerService::new(Arc::new(
        ark_store::ScheduleFileStore::open(&paths.tasks)?,
    )));
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

    let attachments: Arc<dyn ark_ports::AttachmentStore> = Arc::new(AttachmentStore::open(
        &paths.data,
        DEFAULT_MAX_ATTACHMENT_BYTES,
    )?);
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
        skills_config_source(Arc::clone(config), paths.skills.clone()),
        roots.current(),
    ));
    let ui_bus = register_builtin_ui_catalog()?;
    Ok(PlatformServices {
        schedules,
        im,
        attachments,
        providers,
        skills,
        workspace: Arc::new(ark_workspace::WorkspaceInspector::new()),
        mcp: Arc::new(ark_mcp::McpServerRegistry::new(paths.mcp.clone())),
        memory: Arc::new(ark_store::MemoryFileStore::open(&paths.data)?),
        ui_bus,
    })
}

/// Register the built-in Code page and panels on a fresh UI bus.
///
/// Every registration handle is forgotten on purpose: the bus lives for the
/// process lifetime, and dropping a handle would unregister the page or panel.
fn register_builtin_ui_catalog() -> Result<Arc<ark_ui::UiBus>> {
    let ui_bus = Arc::new(ark_ui::UiBus::new());
    let code_page_id = ui_bus.register_page(ark_ui::CODE_UI_PLUGIN_ID, ark_ui::code_page())?;
    code_page_id.forget();
    for (slot, panel) in ark_ui::code_panels() {
        ui_bus
            .register_panel(ark_ui::CODE_UI_PLUGIN_ID, slot, panel)?
            .forget();
    }
    tracing::info!("registered built-in UI catalog");
    Ok(ui_bus)
}

/// Load the local plugin host and the MCP bus its tools register on.
///
/// One MCP bus and one governed HTTP egress serve both plugins and external
/// MCP servers. The host registers plugin tools on the bus while loading, and
/// the tool group below merges them with the external adapters, so there is
/// still exactly one catalog.
///
/// The runtime must outlive assembly: dropping it runs every `Registration`'s
/// `Drop`, which would unregister the pages, panels and tools it published.
///
/// Plugins get no `EventSink`: the platform handle exposes the raw sink, so
/// handing it over would let a plugin append arbitrary events to any thread.
/// Nothing in this version needs it, so the field stays `None`.
fn assemble_plugin_host(
    paths: &RuntimePaths,
    ui_bus: &Arc<ark_ui::UiBus>,
) -> Result<(crate::plugin_host::PluginRuntime, Arc<ark_mcp::McpBus>)> {
    let plugin_mcp = Arc::new(ark_mcp::McpBus::default());
    let plugins = crate::plugin_host::assemble(&paths.data, ui_bus, Arc::clone(&plugin_mcp))?;
    Ok((plugins, plugin_mcp))
}

/// Resolve the effective model client: provider table first, legacy config
/// fallback second.
fn select_model(
    providers: &Arc<ark_provider::ProviderStore>,
    config: &Arc<ConfigStore>,
    usage: Arc<UsageRecorder>,
) -> (Arc<dyn ModelClient>, String) {
    match providers.resolve(None) {
        Some(endpoint) => model::select_from_config(endpoint, Arc::clone(&usage)),
        None => model::select(&config.snapshot(), usage),
    }
}

/// The pieces the parent/child turn machinery share once assembled.
struct AgentSurface {
    delegation: Arc<ark_delegation::DelegationRuntime>,
    tools: Arc<ToolRegistry>,
    completion_notifications: tokio::sync::broadcast::Receiver<ThreadId>,
}

/// Assemble the sub-agent surface (design §7.8).
///
/// Construction order is deliberate:
///
///   1. the **child** registry (builtins only, no `delegate_task`), which is
///      what makes `max_depth = 1` structural rather than a runtime check;
///   2. the executor that drives a child turn with the same reactor;
///   3. the delegation runtime, which owns budget, records and handles;
///   4. the **parent** registry, which adds `delegate_task` on top.
///
/// Building the parent registry last is the only way to avoid a cycle: the
/// tool needs the runner, and the runner needs an executor that needs tools.
async fn assemble_agent_surface(
    ctx: &Assembly,
    platform: &PlatformServices,
    model: &Arc<dyn ModelClient>,
    hooks: &Arc<dyn ark_ports::HookBroker>,
    plugins: &crate::plugin_host::PluginRuntime,
) -> Result<AgentSurface> {
    let child_tools = Arc::new(build_child_tools(
        Arc::clone(&platform.skills),
        Arc::clone(plugins.http()) as Arc<dyn ark_ports::HttpCapability>,
    )?);
    let subagent_policy = Arc::new(crate::child::ConfigSubagentPolicy::new(Arc::clone(
        &ctx.config,
    )));

    let child_executor = Arc::new(crate::child::ChildTurnExecutorImpl::new(
        Arc::clone(&ctx.config),
        Arc::clone(&platform.providers),
        Arc::clone(&ctx.usage),
        Arc::clone(model),
        Arc::clone(&child_tools),
        Arc::clone(&ctx.sink),
        Arc::clone(&ctx.store_port),
        Arc::clone(&ctx.policy),
        Arc::clone(&ctx.roots),
        Arc::clone(&platform.skills),
        Arc::clone(hooks),
    ));
    let delegation = Arc::new(ark_delegation::DelegationRuntime::new(
        Arc::clone(&ctx.store_port),
        Arc::clone(&ctx.sink),
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
        Arc::clone(&platform.skills),
        Arc::clone(plugins.http()) as Arc<dyn ark_ports::HttpCapability>,
        Arc::clone(&ctx.store_port),
    )?);

    Ok(AgentSurface {
        delegation,
        tools,
        completion_notifications,
    })
}

/// Merge the external and plugin tool families into one catalog and wrap the
/// registry in the sandboxed runner every turn shares.
///
/// `mcp.connect_all()` is async because external servers may handshake over
/// the network; it must observe the same registry instance the routes get.
async fn build_tool_runner(
    platform: &PlatformServices,
    agent: &AgentSurface,
    plugins: &crate::plugin_host::PluginRuntime,
    roots: &Arc<WorkspaceRoots>,
    policy: &Arc<dyn SandboxPolicy>,
) -> Result<Arc<RegistryToolRunner>> {
    // One `mcp` group: external server adapters plus local plugin tools. Both
    // families are qualified `<provider>.<tool>`, so a collision is a real
    // configuration conflict and `replace_group` refuses it rather than letting
    // one family silently shadow the other.
    let mut mcp_tools = platform.mcp.connect_all().await;
    mcp_tools.extend(plugins.tools());
    agent.tools.replace_group("mcp", mcp_tools)?;
    Ok(Arc::new(RegistryToolRunner::new(
        Arc::clone(&agent.tools),
        Arc::clone(policy),
        roots.current(),
    )))
}

/// Construct the `AppState` every HTTP route shares, wiring in the adapters
/// the handlers see through `ark-ports` traits.
#[allow(clippy::too_many_arguments)]
fn assemble_app_state(
    args: &ServeArgs,
    paths: &RuntimePaths,
    ctx: &Assembly,
    platform: &PlatformServices,
    agent: &AgentSurface,
    plugins: &crate::plugin_host::PluginRuntime,
    interactive: &Arc<ark_server::InteractiveGates>,
    hooks: &Arc<dyn ark_ports::HookBroker>,
    runner: Arc<AgentTurnRunner>,
    mcp_adapter: &Arc<McpServerAdapter>,
) -> AppState {
    let base = base_services(ctx, platform, plugins, mcp_adapter);

    AppState::new(
        Arc::clone(&ctx.store_port),
        agent.tools.port(),
        Arc::clone(&ctx.config),
        Arc::clone(&ctx.usage),
        runner,
        paths.data.clone(),
        env!("CARGO_PKG_VERSION"),
    )
    .with_config_dir(paths.config.clone())
    .with_attachments(Arc::clone(&platform.attachments))
    .with_base_services(base)
    .with_subagents(
        Arc::clone(&agent.delegation) as Arc<dyn ark_ports::SubagentRunner>,
        Arc::clone(&agent.delegation) as Arc<dyn ark_ports::SubagentRegistry>,
    )
    .with_gates(Arc::clone(interactive))
    .with_schedules(Arc::clone(&platform.schedules))
    .with_im(Arc::clone(&platform.im))
    .with_auth_token(args.auth_token.clone())
    .with_hooks(Arc::clone(hooks))
}

/// Project each platform service onto its `ark-ports` trait for `BaseServices`.
fn base_services(
    ctx: &Assembly,
    platform: &PlatformServices,
    plugins: &crate::plugin_host::PluginRuntime,
    mcp_adapter: &Arc<McpServerAdapter>,
) -> BaseServices {
    BaseServices::new()
        .with_providers(Arc::new(ProviderAdapter::new(Arc::clone(
            &platform.providers,
        ))))
        .with_skills(Arc::new(SkillAdapter::new(Arc::clone(&platform.skills))))
        .with_workspace(Arc::new(WorkspaceAdapter::new(
            Arc::clone(&platform.workspace),
            Arc::clone(&ctx.roots),
        )))
        .with_mcp(Arc::clone(mcp_adapter) as Arc<dyn ark_ports::McpServerService>)
        .with_memory(Arc::new(MemoryAdapter::new(Arc::clone(&platform.memory))))
        .with_ui(Arc::clone(&platform.ui_bus) as Arc<dyn ark_ports::UiCapability>)
        .with_plugins(Arc::clone(plugins.host()) as Arc<dyn ark_ports::PluginHostService>)
        .with_folder_dialog(Arc::new(crate::base::NativeFolderDialog))
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
                    ark_server::schedule_auto_continuation(state.clone(), thread_id).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Recover from the durable inbox through the same safe scan
                    // used at startup, including persisted interrupt suppression.
                    ark_server::sync_auto_continue_policy(state.clone()).await;
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
