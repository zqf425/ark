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
    CompletionInbox, GateBroker, ModelSeed, Reactor, ReactorConfig, ToolCallRequest, TurnEvents,
    TurnLimits, TurnMachine, TurnOutcome,
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

    // The HTTP gate handlers and every parent reactor receive this exact same
    // rendezvous. A request waits until the Code UI resolves it; `AllowAllGates`
    // would make approval cards cosmetic.
    let gates = Arc::new(ark_server::InteractiveGates::new());
    let gate_broker: Arc<dyn GateBroker> = gates.clone();

    let runner = Arc::new(AgentTurnRunner {
        providers: Arc::clone(&providers),
        config: Arc::clone(&config),
        usage: Arc::clone(&usage),
        tools: tool_runner,
        store: Arc::clone(&store_port),
        sink,
        policy,
        roots: Arc::clone(&roots),
        tool_schemas: tools.schemas(),
        skills: Arc::clone(&skills),
        gates: gate_broker,
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
    .with_gates(gates)
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
    /// Kept so a turn can resolve its model from the **current** provider table
    /// and config snapshot. Holding one client for the process lifetime made
    /// `Applied` a lie: editing a provider or `tuning` never reached the next
    /// parent turn (design §7.15, P1-7).
    providers: Arc<ark_provider::ProviderStore>,
    config: Arc<ConfigStore>,
    usage: Arc<UsageRecorder>,
    tools: Arc<RegistryToolRunner>,
    store: Arc<dyn SessionStore>,
    sink: Arc<dyn EventSink>,
    policy: Arc<dyn SandboxPolicy>,
    /// Resolved per turn so a workspace chosen in Settings applies immediately.
    roots: Arc<WorkspaceRoots>,
    tool_schemas: Vec<serde_json::Value>,
    /// Skill runtime, consulted once per turn for activated instructions.
    skills: Arc<ark_skills::SkillRuntime>,
    /// Human gate broker shared with the HTTP command surface.
    gates: Arc<dyn GateBroker>,
}

impl AgentTurnRunner {
    /// Model client for this turn, from the live provider table.
    ///
    /// The provider table wins; `model_providers` in `config.json` remains the
    /// legacy fallback. Rebuilt per turn so an added/edited provider applies to
    /// the next turn without a restart.
    fn model_for_turn(
        &self,
        snapshot: &RuntimeConfig,
        provider: Option<ark_llm::ProviderConfig>,
    ) -> Arc<dyn ModelClient> {
        match provider {
            Some(endpoint) => model::select_from_config(endpoint, Arc::clone(&self.usage)).0,
            None => model::select(snapshot, Arc::clone(&self.usage)).0,
        }
    }

    /// Per-turn limits from the live `tuning` block.
    ///
    /// A running turn keeps the snapshot it started with; the next turn sees the
    /// edit. That is the bounded-lifetime semantics the design asks for.
    fn limits_for_turn(
        snapshot: &RuntimeConfig,
        provider: Option<&ark_llm::ProviderConfig>,
    ) -> TurnLimits {
        let tuning = &snapshot.tuning;
        let context_limit_tokens =
            model::context_limit_tokens(tuning.context_limit_tokens, provider);
        TurnLimits {
            max_rounds: tuning.max_rounds.max(1),
            max_tokens: tuning.max_tokens.max(1),
            max_tool_calls: tuning.max_tool_calls.max(1),
            max_identical_calls: tuning.max_identical_calls.max(1),
            context_limit_tokens,
            keep_recent_messages: tuning.keep_recent_messages.max(1),
            max_compactions: TurnLimits::default().max_compactions,
        }
    }

    /// Model-delta channel capacity from `tuning`, falling back to the default.
    fn chunk_buffer_for_turn(snapshot: &RuntimeConfig) -> usize {
        let tuned = snapshot.tuning.chunk_buffer;
        if tuned == 0 {
            CHUNK_BUFFER
        } else {
            tuned
        }
    }
}

/// Read the durable completion inbox before constructing a parent turn.
///
/// Child completion is at-least-once delivery: if acknowledgement fails after
/// the model turn, the next parent turn safely receives the same source again.
async fn completion_inbox(
    store: &dyn SessionStore,
    thread_id: ThreadId,
) -> Result<CompletionInbox> {
    let events = store.replay_child_completion_events(thread_id).await?;
    Ok(CompletionInbox::from_events(&events))
}

/// Persists the completion IDs delivered to a parent turn without changing its
/// completed outcome when the acknowledgement write itself fails.
async fn acknowledge_completion_inbox(
    sink: &dyn EventSink,
    thread_id: ThreadId,
    turn_id: TurnId,
    notification_ids: &[String],
) {
    for notification_id in notification_ids {
        if let Err(error) = sink
            .publish(
                thread_id,
                ark_contract::RuntimeEvent::ChildCompletionAcknowledged {
                    seq: 0,
                    thread_id,
                    turn_id,
                    timestamp: chrono::Utc::now(),
                    notification_id: notification_id.clone(),
                },
            )
            .await
        {
            tracing::warn!(
                thread = %thread_id,
                turn = %turn_id,
                %notification_id,
                %error,
                "failed to acknowledge child completion; it will be delivered again"
            );
        }
    }
}

/// Converts a reactor outcome into the server-facing turn result.
///
/// * `outcome`: terminal result emitted by the parent reactor.
/// * `sink`: durable event destination for successful inbox acknowledgements.
/// * `thread_id` / `turn_id`: identity of the completed parent turn.
/// * `inbox`: completion IDs supplied to that turn as a system source.
/// * Returns: the terminal result without changing a failed or aborted turn.
async fn finish_parent_turn(
    outcome: TurnOutcome,
    sink: &dyn EventSink,
    thread_id: ThreadId,
    turn_id: TurnId,
    inbox: &CompletionInbox,
) -> TurnEnd {
    match outcome {
        TurnOutcome::Completed => {
            acknowledge_completion_inbox(sink, thread_id, turn_id, inbox.notification_ids()).await;
            TurnEnd::Completed
        }
        TurnOutcome::Failed { message } => TurnEnd::Failed(message),
        TurnOutcome::Aborted { reason } => TurnEnd::Aborted(reason),
    }
}

#[async_trait::async_trait]
impl TurnRunner for AgentTurnRunner {
    async fn run_turn(&self, job: TurnJob) -> Result<TurnEnd> {
        let inbox = completion_inbox(self.store.as_ref(), job.thread_id).await?;
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
        if let Some(message) = inbox.system_message() {
            seed_messages.push(message);
        }
        // One snapshot per turn: every value below is derived from the same
        // config state, so a turn cannot mix a new model with old limits.
        let snapshot = self.config.snapshot();
        let provider = self.providers.resolve(None);
        let model = self.model_for_turn(&snapshot, provider.clone());
        let mut reactor = Reactor::new(ReactorConfig {
            model,
            tools,
            writer,
            gates: Arc::clone(&self.gates),
            sandbox: Arc::clone(&self.policy),
            events,
            cancel: job.cancel,
            seed: ModelSeed {
                messages: seed_messages,
                tools: self.tool_schemas.clone(),
                temperature: provider.as_ref().and_then(|provider| provider.temperature),
                max_tokens: provider.as_ref().and_then(|provider| provider.max_tokens),
            },
            machine: TurnMachine::new(Self::limits_for_turn(&snapshot, provider.as_ref())),
            chunk_buffer: Self::chunk_buffer_for_turn(&snapshot),
        })
        .with_steering(job.steering);
        reactor.push_user(job.text);
        let outcome = reactor.run().await?;
        Ok(finish_parent_turn(
            outcome,
            self.sink.as_ref(),
            job.thread_id,
            job.turn_id,
            &inbox,
        )
        .await)
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
        // The reactor already decided the sandbox mode for this call and carried
        // it here; re-deciding would flip an approved `bash` back to
        // `NeedsApproval` (design §7.3, P1-5). `run_in` remains the fallback for
        // calls that never went through the model path.
        match call.sandbox {
            Some(decision) => {
                self.inner
                    .run_decided(
                        self.roots.current(),
                        self.thread_id,
                        self.turn_id,
                        &call.tool,
                        call.arguments.clone(),
                        decision,
                    )
                    .await
            }
            None => {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ark_common::ChildId;
    use ark_contract::RuntimeEvent;
    use ark_llm::ProviderConfig;
    use chrono::Utc;

    struct TestDataDir(std::path::PathBuf);

    impl TestDataDir {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time is after the Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("ark-completion-inbox-{nonce}"));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDataDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn child_completion_events_are_loaded_as_the_next_turn_system_source() {
        let data_dir = TestDataDir::new();
        let store = FileSessionStore::open(StoreConfig::new(&data_dir.0)).unwrap();
        let thread = store.create_thread("parent").await.unwrap().thread_id;
        store
            .append_event(
                thread,
                RuntimeEvent::ChildCompleted {
                    seq: 0,
                    thread_id: thread,
                    timestamp: Utc::now(),
                    notification_id: "child:one".to_string(),
                    child_id: ChildId::new(),
                    label: "inspect runtime".to_string(),
                    status: "completed".to_string(),
                    summary: Some("The runtime is healthy.".to_string()),
                    error: None,
                    usage: None,
                    tool_invocations: None,
                },
            )
            .await
            .unwrap();

        let inbox = completion_inbox(&store, thread).await.unwrap();
        assert_eq!(inbox.notification_ids(), &["child:one".to_string()]);
        assert_eq!(
            inbox.system_message().unwrap().role,
            ark_ports::ModelRole::System
        );
    }

    #[tokio::test]
    async fn completed_parent_turn_acknowledges_the_injected_completion_and_skips_it_next_time() {
        let data_dir = TestDataDir::new();
        let store = FileSessionStore::open(StoreConfig::new(&data_dir.0)).unwrap();
        let thread = store.create_thread("parent").await.unwrap().thread_id;
        let turn = TurnId::new();

        store
            .append_event(
                thread,
                RuntimeEvent::ChildCompleted {
                    seq: 0,
                    thread_id: thread,
                    timestamp: Utc::now(),
                    notification_id: "child:one".to_string(),
                    child_id: ChildId::new(),
                    label: "inspect runtime".to_string(),
                    status: "completed".to_string(),
                    summary: Some("The runtime is healthy.".to_string()),
                    error: None,
                    usage: None,
                    tool_invocations: None,
                },
            )
            .await
            .unwrap();
        let inbox = completion_inbox(&store, thread).await.unwrap();
        assert!(
            inbox.system_message().is_some(),
            "the turn receives a system source"
        );

        let end = finish_parent_turn(TurnOutcome::Completed, &store, thread, turn, &inbox).await;

        assert!(matches!(end, TurnEnd::Completed));
        assert!(completion_inbox(&store, thread)
            .await
            .unwrap()
            .notification_ids()
            .is_empty());
    }

    #[tokio::test]
    async fn failed_parent_turn_leaves_the_completion_for_redelivery() {
        let data_dir = TestDataDir::new();
        let store = FileSessionStore::open(StoreConfig::new(&data_dir.0)).unwrap();
        let thread = store.create_thread("parent").await.unwrap().thread_id;
        store
            .append_event(
                thread,
                RuntimeEvent::ChildCompleted {
                    seq: 0,
                    thread_id: thread,
                    timestamp: Utc::now(),
                    notification_id: "child:one".to_string(),
                    child_id: ChildId::new(),
                    label: "inspect runtime".to_string(),
                    status: "failed".to_string(),
                    summary: None,
                    error: Some("child failed".to_string()),
                    usage: None,
                    tool_invocations: None,
                },
            )
            .await
            .unwrap();
        let inbox = completion_inbox(&store, thread).await.unwrap();

        let end = finish_parent_turn(
            TurnOutcome::Failed {
                message: "model failed".to_string(),
            },
            &store,
            thread,
            TurnId::new(),
            &inbox,
        )
        .await;

        assert!(matches!(end, TurnEnd::Failed(message) if message == "model failed"));
        assert_eq!(
            completion_inbox(&store, thread)
                .await
                .unwrap()
                .notification_ids(),
            &["child:one".to_string()]
        );
    }

    #[test]
    fn provider_context_window_caps_the_next_turn_and_reserves_requested_output() {
        let mut config = RuntimeConfig::default();
        config.tuning.context_limit_tokens = 100_000;
        let mut provider = ProviderConfig::new("p", "m");
        provider.context_window_tokens = Some(32_000);
        provider.max_tokens = Some(4_096);

        let limits = AgentTurnRunner::limits_for_turn(&config, Some(&provider));

        assert_eq!(limits.context_limit_tokens, 27_904);
    }
}
