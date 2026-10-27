//! Explicit composition root (design §5.3).
//!
//! Every concrete implementation is named here and nowhere else. The rule the
//! design asks for — no `if plugin_id == ...` — is enforced by shape: each
//! collaborator is a field of [`Wiring`], injected into the pieces that need it,
//! and nothing downstream can reach for a different implementation.

use std::sync::Arc;

use ark_agent::exec::journal::EventSinkWriter;
use ark_agent::exec::tool_runner::ToolRunner as AgentToolRunner;
use ark_agent::{
    AllowAllGates, ModelSeed, Reactor, ReactorConfig, ToolCallRequest, TurnEvents, TurnLimits,
    TurnMachine, TurnOutcome,
};
use ark_common::{Result, ThreadId, TurnId};
use ark_config::{ConfigStore, RuntimeConfig};
use ark_obs::UsageRecorder;
use ark_ports::{
    BaseServices, EventSink, ModelClient, ModelMessage, SandboxPolicy, SessionStore, ToolOutput,
    ToolRegistryPort,
};
use ark_store::{FileSessionStore, StoreConfig};
use ark_tool::{Autonomy, BuiltinSandboxPolicy, RegistryToolRunner, ToolRegistry};

use ark_server::{AppState, TurnEnd, TurnJob, TurnRunner};

use crate::base::{McpServerAdapter, ProviderAdapter, SkillAdapter, WorkspaceAdapter};
use crate::cli::ServeArgs;
use crate::model;
use crate::workspace::WorkspaceRoots;

/// System prefix. Static text only, so the prompt-cache prefix stays byte-stable.
pub const SYSTEM_PROMPT: &str = "\
You are ark, a coding agent inside the smart-ark desktop application.
Work in small, verifiable steps. Prefer reading the repository over guessing.
Every tool call is logged; never claim a check you did not run.";

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
}

/// Build the runtime for `args`.
pub async fn build(args: &ServeArgs) -> Result<Wiring> {
    std::fs::create_dir_all(&args.data_dir)?;
    let config_path = ark_config::config_path(&args.data_dir);
    let loaded = ark_config::load_or_default(&config_path).map_err(|message| {
        ark_common::ArkError::validation(format!(
            "cannot load {}: {message}",
            config_path.display()
        ))
    })?;
    let runtime_config = RuntimeConfig {
        port: args.port,
        data_dir: args.data_dir.clone(),
        ..loaded
    };
    let config = Arc::new(ConfigStore::new(runtime_config));

    // Real store: the session directory is the system of record.
    let store = Arc::new(FileSessionStore::open(StoreConfig::new(&args.data_dir))?);
    store.reconcile()?;
    let store_port: Arc<dyn SessionStore> = store.clone();
    let sink: Arc<dyn EventSink> = store.clone();

    // The sandbox policy is shared by parent and child turns: a delegated task
    // must never get a laxer sandbox than the turn that spawned it.
    let policy: Arc<dyn SandboxPolicy> =
        Arc::new(BuiltinSandboxPolicy::new(Autonomy::WorkspaceWrite));
    // Workspace roots are resolved per call from the live config snapshot, so a
    // change made in Settings takes effect on the next tool call instead of
    // requiring a restart (design §7.15: `default_workspace` is `Hot`).
    let roots = Arc::new(WorkspaceRoots::new(
        Arc::clone(&config),
        args.workspace.clone(),
    ));
    let usage = Arc::new(UsageRecorder::new());

    // ---- platform services (design §7.5.4) ----
    //
    // Built here and only here. Each store owns its own file under `data_dir`,
    // and the HTTP layer sees them through the `ark-ports` traits below.
    let providers = Arc::new(ark_provider::ProviderStore::new(args.data_dir.clone()));
    let skills = Arc::new(ark_skills::SkillRuntime::new(
        ark_skills::SkillsConfig::default(),
        roots.current(),
    ));
    let workspace = Arc::new(ark_workspace::WorkspaceInspector::new());
    let mcp = Arc::new(ark_mcp::McpServerRegistry::new(args.data_dir.clone()));

    // The provider table is the source of truth for the model client, so a
    // provider added through the GUI takes effect on the next turn instead of
    // requiring a restart. `model_providers` in config.json stays honoured as a
    // legacy fallback for a hand-written config.
    let provider_endpoint = providers.resolve(None);
    let (model, model_label) = match provider_endpoint {
        Some(endpoint) => model::select_from_config(endpoint, Arc::clone(&usage)),
        None => model::select(&config.snapshot(), Arc::clone(&usage)),
    };

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
    let child_tools = Arc::new(build_child_tools()?);
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
    ));
    let delegation = Arc::new(ark_delegation::DelegationRuntime::new(
        Arc::clone(&store_port),
        Arc::clone(&sink),
        subagent_policy,
        child_executor,
    ));
    // Load the authoritative `children/runs.jsonl` records before the HTTP
    // surface exists. Without this the in-memory registry starts empty, so every
    // historical child would answer 404 after a restart even though its record
    // and session are on disk (design §7.4.1). It also converges runs that were
    // still queued/running when the previous process died.
    let hydrated = delegation.hydrate().await?;
    tracing::info!(children = hydrated, "loaded child-run records");

    let tools = Arc::new(build_tools(Arc::clone(&delegation))?);
    let tool_runner = Arc::new(RegistryToolRunner::new(
        Arc::clone(&tools),
        Arc::clone(&policy),
        roots.current(),
    ));

    let runner = Arc::new(AgentTurnRunner {
        model,
        tools: tool_runner,
        sink,
        policy,
        roots: Arc::clone(&roots),
        tool_schemas: tools.schemas(),
        skills: Arc::clone(&skills),
    });

    let base = BaseServices::new()
        .with_providers(Arc::new(ProviderAdapter::new(Arc::clone(&providers))))
        .with_skills(Arc::new(SkillAdapter::new(Arc::clone(&skills))))
        .with_workspace(Arc::new(WorkspaceAdapter::new(Arc::clone(&workspace))))
        .with_mcp(Arc::new(McpServerAdapter::new(Arc::clone(&mcp))));

    let state = AppState::new(
        store_port,
        tools.port(),
        Arc::clone(&config),
        Arc::clone(&usage),
        runner,
        args.data_dir.clone(),
        env!("CARGO_PKG_VERSION"),
    )
    .with_base_services(base)
    .with_subagents(
        Arc::clone(&delegation) as Arc<dyn ark_ports::SubagentRunner>,
        Arc::clone(&delegation) as Arc<dyn ark_ports::SubagentRegistry>,
    )
    .with_auth_token(args.auth_token.clone());

    Ok(Wiring {
        state,
        store,
        tools,
        config,
        usage,
        model_label,
    })
}

/// Register every builtin plus `delegate_task` for the parent agent.
///
/// A conflict here is a programming error.
fn build_tools(runner: Arc<ark_delegation::DelegationRuntime>) -> Result<ToolRegistry> {
    let mut registry = build_child_tools()?;
    registry.register_all(ark_tool::delegation_tools(runner))?;
    Ok(registry)
}

/// The child tool set: builtins only.
///
/// `delegate_task` is absent on purpose. A child therefore has no way to spawn a
/// grandchild, which enforces `max_depth = 1` without trusting a runtime check
/// that a future refactor could bypass.
fn build_child_tools() -> Result<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry.register_all(ark_tool::builtins())?;
    Ok(registry)
}

/// One turn, driven by the real reactor (`ark-agent`), with the real store and
/// the real tool registry behind the ports.
struct AgentTurnRunner {
    model: Arc<dyn ModelClient>,
    tools: Arc<RegistryToolRunner>,
    sink: Arc<dyn EventSink>,
    policy: Arc<dyn SandboxPolicy>,
    /// Resolved per turn so a workspace chosen in Settings applies immediately.
    roots: Arc<WorkspaceRoots>,
    tool_schemas: Vec<serde_json::Value>,
    /// Skill runtime, consulted once per turn for activated instructions.
    skills: Arc<ark_skills::SkillRuntime>,
}

#[async_trait::async_trait]
impl TurnRunner for AgentTurnRunner {
    async fn run_turn(&self, job: TurnJob) -> Result<TurnEnd> {
        let events = TurnEvents::new(job.thread_id, job.turn_id);
        let writer = Arc::new(EventSinkWriter::new(Arc::clone(&self.sink), events));
        let tools: Arc<dyn AgentToolRunner> = Arc::new(ToolRunnerAdapter {
            inner: Arc::clone(&self.tools),
            roots: Arc::clone(&self.roots),
            thread_id: job.thread_id,
            turn_id: job.turn_id,
        });
        // Skill instructions ride the *dynamic* tail, never the static prefix, so
        // the prompt-cache prefix stays byte-identical between turns (design
        // §7.4.6 / §7.14). Only activated skills are injected; the rest appear in
        // the catalog line so the model knows they exist.
        let resolution = self.skills.resolve_for_turn(&job.text);
        let mut seed_messages = vec![
            ModelMessage::system(SYSTEM_PROMPT),
            ModelMessage::system(workspace_line(&self.roots.current())),
        ];
        for instruction in &resolution.instructions {
            seed_messages.push(ModelMessage::system(format!(
                "[skill instructions]\n{instruction}"
            )));
        }
        if let Some(catalog) = &resolution.catalog {
            seed_messages.push(ModelMessage::system(format!(
                "[available skills]\n{catalog}"
            )));
        }
        let mut reactor = Reactor::new(ReactorConfig {
            model: Arc::clone(&self.model),
            tools,
            writer,
            gates: Arc::new(AllowAllGates),
            sandbox: Arc::clone(&self.policy),
            events,
            cancel: job.cancel,
            seed: ModelSeed {
                messages: seed_messages,
                tools: self.tool_schemas.clone(),
                temperature: None,
                max_tokens: None,
            },
            machine: TurnMachine::new(TurnLimits::default()),
            chunk_buffer: CHUNK_BUFFER,
        });
        reactor.push_user(job.text);
        let outcome = reactor.run().await?;
        Ok(match outcome {
            TurnOutcome::Completed => TurnEnd::Completed,
            TurnOutcome::Failed { message } => TurnEnd::Failed(message),
            TurnOutcome::Aborted { reason } => TurnEnd::Aborted(reason),
        })
    }
}

/// One-line workspace fact. Kept out of the static prefix on purpose: the cache
/// prefix must stay byte-identical between turns and machines.
fn workspace_line(workspace: &std::path::Path) -> String {
    format!("workspace root: {}", workspace.display())
}

/// Adapter from `ark-tool`'s runner to `ark-agent`'s `ToolRunner` trait.
struct ToolRunnerAdapter {
    inner: Arc<RegistryToolRunner>,
    roots: Arc<WorkspaceRoots>,
    thread_id: ThreadId,
    turn_id: TurnId,
}

#[async_trait::async_trait]
impl AgentToolRunner for ToolRunnerAdapter {
    async fn execute(&self, call: &ToolCallRequest) -> Result<ToolOutput> {
        self.inner
            .run_in(
                self.roots.current(),
                self.thread_id,
                self.turn_id,
                &call.tool,
                call.arguments.clone(),
            )
            .await
    }
}
