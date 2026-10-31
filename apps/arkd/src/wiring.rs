//! Explicit composition root (design §5.3).
//!
//! Every concrete implementation is named here and nowhere else. The rule the
//! design asks for — no `if plugin_id == ...` — is enforced by shape: each
//! collaborator is a field of [`Wiring`], injected into the pieces that need it,
//! and nothing downstream can reach for a different implementation.

use std::collections::HashSet;
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
    BaseServices, EventSink, ModelClient, ModelMessage, ModelRole, SandboxPolicy, SessionStore,
    ToolCallRef, ToolOutput, ToolRegistryPort,
};
use ark_store::{AttachmentStore, FileSessionStore, StoreConfig, DEFAULT_MAX_ATTACHMENT_BYTES};
use ark_tool::{Autonomy, BuiltinSandboxPolicy, RegistryToolRunner, ToolRegistry};

use ark_server::{AppState, TurnEnd, TurnJob, TurnRunner, TurnSource};

use crate::base::{
    McpServerAdapter, MemoryAdapter, ProviderAdapter, SkillAdapter, WorkspaceAdapter,
};
use crate::cli::ServeArgs;
use crate::load_skill::LoadSkillTool;
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
    let attachments = Arc::new(AttachmentStore::open(
        &args.data_dir,
        DEFAULT_MAX_ATTACHMENT_BYTES,
    )?);
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
    let roots = Arc::new(WorkspaceRoots::new(args.workspace.clone(), &args.data_dir)?);
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
    let memory = Arc::new(ark_store::MemoryFileStore::open(&args.data_dir)?);

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
    let child_tools = Arc::new(build_child_tools(Arc::clone(&skills))?);
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
        Arc::clone(&skills),
    )?);
    tools.replace_group("mcp", mcp.connect_all().await)?;
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
        roots: Arc::clone(&roots),
        tool_registry: Arc::clone(&tools),
        skills: Arc::clone(&skills),
        memory: Arc::clone(&memory),
        gates: gate_broker,
    });

    let base = BaseServices::new()
        .with_providers(Arc::new(ProviderAdapter::new(Arc::clone(&providers))))
        .with_skills(Arc::new(SkillAdapter::new(Arc::clone(&skills))))
        .with_workspace(Arc::new(WorkspaceAdapter::new(
            Arc::clone(&workspace),
            Arc::clone(&roots),
        )))
        .with_mcp(Arc::new(McpServerAdapter::new(
            Arc::clone(&mcp),
            Arc::clone(&tools),
        )))
        .with_memory(Arc::new(MemoryAdapter::new(Arc::clone(&memory))))
        .with_ui(Arc::clone(&ui_bus) as Arc<dyn ark_ports::UiCapability>);

    let state = AppState::new(
        store_port,
        tools.port(),
        Arc::clone(&config),
        Arc::clone(&usage),
        runner,
        args.data_dir.clone(),
        env!("CARGO_PKG_VERSION"),
    )
    .with_attachments(attachments)
    .with_base_services(base)
    .with_subagents(
        Arc::clone(&delegation) as Arc<dyn ark_ports::SubagentRunner>,
        Arc::clone(&delegation) as Arc<dyn ark_ports::SubagentRegistry>,
    )
    .with_gates(gates)
    .with_auth_token(args.auth_token.clone());

    spawn_completion_listener(state.clone(), completion_notifications);
    // Recover unacknowledged completions that were durable before this process
    // started (for example, a crash after `child_completed` but before debounce).
    ark_server::service::sync_auto_continue_policy(state.clone()).await;

    Ok(Wiring {
        state,
        store,
        tools,
        config,
        usage,
        model_label,
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

/// Register every builtin plus `delegate_task` for the parent agent.
///
/// A conflict here is a programming error.
fn build_tools(
    runner: Arc<dyn ark_ports::SubagentRunner>,
    skills: Arc<ark_skills::SkillRuntime>,
) -> Result<ToolRegistry> {
    let registry = build_child_tools(skills)?;
    registry.register_all(ark_tool::delegation_tools(runner))?;
    Ok(registry)
}

/// The child tool set: builtins only.
///
/// `delegate_task` is absent on purpose. A child therefore has no way to spawn a
/// grandchild, which enforces `max_depth = 1` without trusting a runtime check
/// that a future refactor could bypass.
fn build_child_tools(skills: Arc<ark_skills::SkillRuntime>) -> Result<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register_all(ark_tool::builtins())?;
    registry.register(Arc::new(LoadSkillTool::new(skills)))?;
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
    /// Resolved per turn so a workspace chosen in Settings applies immediately.
    roots: Arc<WorkspaceRoots>,
    tool_registry: Arc<ToolRegistry>,
    /// Skill runtime, consulted once per turn for activated instructions.
    skills: Arc<ark_skills::SkillRuntime>,
    /// Long-term memory document. Re-read once per turn so a memory the user
    /// just saved is visible to the very next turn without a restart.
    memory: Arc<ark_store::MemoryFileStore>,
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

/// Auto-continuations project the inbox with the per-notification delivery
/// bound: notifications already auto-delivered MAX_AUTO_DELIVERY_ATTEMPTS
/// times are withheld, so a poison notification cannot loop the supervisor.
/// They remain unacknowledged, so explicit user turns still receive them.
async fn auto_continuation_inbox(
    store: &dyn SessionStore,
    thread_id: ThreadId,
) -> Result<CompletionInbox> {
    let events = store.replay_child_completion_events(thread_id).await?;
    Ok(CompletionInbox::from_events_for_auto_continuation(&events))
}

/// Terminates parent turns whose process-owned reactor vanished on restart.
///
/// The event log is the sole durable source. A reactor, its cancellation token,
/// steering queue and gate waiters do not survive a process boundary, so this
/// intentionally does not recreate any of them or replay a tool/model call.
async fn abort_interrupted_turns(store: &dyn SessionStore) -> Result<usize> {
    let mut aborted = 0;
    for thread in store.list_threads().await? {
        let mut open = HashSet::new();
        for event in store.replay_interaction_events(thread.thread_id).await? {
            match event {
                ark_contract::RuntimeEvent::TurnStarted { turn_id, .. } => {
                    open.insert(turn_id);
                }
                ark_contract::RuntimeEvent::TurnCompleted { turn_id, .. }
                | ark_contract::RuntimeEvent::TurnFailed { turn_id, .. }
                | ark_contract::RuntimeEvent::TurnAborted { turn_id, .. } => {
                    open.remove(&turn_id);
                }
                _ => {}
            }
        }
        for turn_id in open {
            store
                .append_event(
                    thread.thread_id,
                    ark_contract::RuntimeEvent::TurnAborted {
                        seq: 0,
                        thread_id: thread.thread_id,
                        turn_id,
                        timestamp: chrono::Utc::now(),
                        reason: "runtime restarted before turn could finish".to_string(),
                    },
                )
                .await?;
            aborted += 1;
        }
    }
    Ok(aborted)
}

/// Persists the completion IDs delivered to a parent turn without changing its
/// completed outcome when the acknowledgement write itself fails.
async fn acknowledge_completion_inbox(
    sink: &dyn EventSink,
    thread_id: ThreadId,
    turn_id: TurnId,
    notification_ids: &[String],
) -> bool {
    let mut all_acknowledged = true;
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
            all_acknowledged = false;
            tracing::warn!(
                thread = %thread_id,
                turn = %turn_id,
                %notification_id,
                %error,
                "failed to acknowledge child completion; it will be delivered again"
            );
        }
    }
    all_acknowledged
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
    store: &dyn SessionStore,
    sink: &dyn EventSink,
    thread_id: ThreadId,
    turn_id: TurnId,
    inbox: &CompletionInbox,
) -> TurnEnd {
    match outcome {
        TurnOutcome::Completed => {
            let acknowledged =
                acknowledge_completion_inbox(sink, thread_id, turn_id, inbox.notification_ids())
                    .await;
            if acknowledged
                && completion_inbox(store, thread_id)
                    .await
                    .is_ok_and(|remaining| !remaining.notification_ids().is_empty())
            {
                TurnEnd::CompletedWithPendingCompletions
            } else {
                TurnEnd::Completed
            }
        }
        TurnOutcome::Failed {
            message,
            model_request_failed,
        } => TurnEnd::Failed {
            message,
            model_request_failed,
        },
        TurnOutcome::Aborted { reason } => TurnEnd::Aborted(reason),
    }
}

#[async_trait::async_trait]
impl TurnRunner for AgentTurnRunner {
    async fn run_turn(&self, job: TurnJob) -> Result<TurnEnd> {
        let is_auto = matches!(&job.source, TurnSource::AutoContinuation);
        let inbox = if is_auto {
            auto_continuation_inbox(self.store.as_ref(), job.thread_id).await?
        } else {
            completion_inbox(self.store.as_ref(), job.thread_id).await?
        };
        let history = conversation_history(&self.store.read_items(job.thread_id).await?);
        let thread = self.store.get_thread(job.thread_id).await?;
        let (source_text, user_text) = match job.source {
            TurnSource::User(text) => (text.clone(), Some(text)),
            TurnSource::AutoContinuation => (String::new(), None),
        };
        // Capture once before building either the prompt or tool adapter. A
        // per-session override wins, otherwise the process workspace lease is
        // used; neither can change during this turn.
        let workspace = match thread.runtime.workspace.as_deref() {
            Some(raw) => {
                let path = std::path::PathBuf::from(raw);
                if !path.is_dir() {
                    return Err(ark_common::ArkError::validation(format!(
                        "thread workspace does not exist: {}",
                        path.display()
                    )));
                }
                path.canonicalize()
                    .map(|path| {
                        std::path::PathBuf::from(ark_common::path::strip_verbatim(
                            &path.to_string_lossy(),
                        ))
                    })
                    .map_err(|_| {
                        ark_common::ArkError::validation(format!(
                            "cannot canonicalize thread workspace: {}",
                            path.display()
                        ))
                    })?
            }
            None => self.roots.lease().canonical_root,
        };
        let snapshot = self.config.snapshot();
        let policy: Arc<dyn SandboxPolicy> = Arc::new(BuiltinSandboxPolicy::new(
            autonomy_from_override_or_default(thread.runtime.autonomy.as_deref(), &snapshot)?,
        ));
        // Auto-continuations are marked on the durable `turn_started` event so
        // the per-notification delivery-attempt accounting survives restarts.
        let events = if is_auto {
            TurnEvents::auto_continuation(job.thread_id, job.turn_id)
        } else {
            TurnEvents::new(job.thread_id, job.turn_id)
        };
        let writer = Arc::new(EventSinkWriter::new(Arc::clone(&self.sink), events));
        let tools: Arc<dyn AgentToolRunner> = Arc::new(ToolRunnerAdapter {
            inner: Arc::clone(&self.tools),
            workspace: workspace.clone(),
            thread_id: job.thread_id,
            turn_id: job.turn_id,
        });
        // Skill instructions ride the *dynamic* tail, never the static prefix, so
        // the prompt-cache prefix stays byte-identical between turns (design
        // §7.4.6 / §7.14). Only activated skills are injected; the rest appear in
        // the catalog line so the model knows they exist.
        let resolution = self.skills.resolve_for_turn(&source_text);
        let mut seed_messages = vec![
            ModelMessage::system(SYSTEM_PROMPT),
            ModelMessage::system(workspace_line(&workspace)),
        ];
        seed_messages.extend(history);
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
        // Memory also rides the dynamic tail: it changes whenever the user edits
        // a memory, so putting it in the static prefix would invalidate the
        // prompt cache on every such edit.
        if let Some(message) = crate::memory_seed::seed_for_turn(self.memory.as_ref()) {
            seed_messages.push(message);
        }
        if let Some(message) = inbox.system_message() {
            seed_messages.push(message);
        }
        // One snapshot per turn: every value below is derived from the same
        // config state, so a turn cannot mix a new model with old limits.
        let mut provider = match thread.runtime.provider_id.as_deref() {
            Some(id) => Some(self.providers.resolve(Some(id)).ok_or_else(|| {
                ark_common::ArkError::validation(format!("thread provider is unavailable: {id}"))
            })?),
            None => self.providers.resolve(None),
        };
        if let (Some(provider), Some(model)) = (&mut provider, thread.runtime.model) {
            provider.model = model;
        }
        let model = self.model_for_turn(&snapshot, provider.clone());
        let mut reactor = Reactor::new(ReactorConfig {
            model,
            tools,
            writer,
            gates: Arc::clone(&self.gates),
            sandbox: policy,
            events,
            cancel: job.cancel,
            seed: ModelSeed {
                messages: seed_messages,
                tools: self.tool_registry.schemas(),
                temperature: provider.as_ref().and_then(|provider| provider.temperature),
                max_tokens: provider.as_ref().and_then(|provider| provider.max_tokens),
            },
            machine: TurnMachine::new(Self::limits_for_turn(&snapshot, provider.as_ref())),
            chunk_buffer: Self::chunk_buffer_for_turn(&snapshot),
        })
        .with_steering(job.steering);
        if let Some(text) = user_text {
            reactor.push_user(text);
        }
        let outcome = reactor.run().await?;
        Ok(finish_parent_turn(
            outcome,
            self.store.as_ref(),
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

pub(crate) fn autonomy_from_override(value: Option<&str>) -> ark_common::Result<Autonomy> {
    match value {
        Some("workspace_write") => Ok(Autonomy::WorkspaceWrite),
        Some("strict") => Ok(Autonomy::Strict),
        Some("unattended") => Ok(Autonomy::Unattended),
        Some(value) => Err(ark_common::ArkError::validation(format!(
            "invalid autonomy: {value}"
        ))),
        None => Err(ark_common::ArkError::validation("autonomy is missing")),
    }
}

pub(crate) fn autonomy_from_override_or_default(
    value: Option<&str>,
    config: &RuntimeConfig,
) -> ark_common::Result<Autonomy> {
    autonomy_from_override(value.or(Some(&config.default_autonomy)))
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

/// Rebuild the durable conversational prefix for the next model turn.
///
/// Reasoning is deliberately excluded: it is renderer-only and must never be
/// replayed as user-visible assistant text. Completed tool calls are folded
/// into the assistant history as `tool_calls`, each followed by a `ToolRole`
/// result message. The store folds `ToolCallStarted` + `ToolCallFinished` into
/// one `ToolCall` item whose payload carries `call_id`/`tool`/`arguments` and
/// the embedded `output`; terminal items replay the full call+result pair.
/// Non-terminal items are dropped because providers reject requests whose
/// tool calls have no result.
fn conversation_history(items: &[ark_contract::ThreadItem]) -> Vec<ModelMessage> {
    let mut history: Vec<ModelMessage> = Vec::new();
    for item in items {
        match item.kind {
            ark_contract::item::ItemKind::UserMessage => {
                if let Some(text) = nonempty(item.text.as_deref()) {
                    history.push(ModelMessage::user(text));
                }
            }
            ark_contract::item::ItemKind::AssistantMessage => {
                if let Some(text) = nonempty(item.text.as_deref()) {
                    history.push(ModelMessage::assistant(text));
                }
            }
            ark_contract::item::ItemKind::Notice => {
                if let Some(text) = item
                    .text
                    .as_deref()
                    .filter(|text| text.starts_with("[context checkpoint]"))
                {
                    history.push(ModelMessage::system(text));
                }
            }
            ark_contract::item::ItemKind::ToolCall => {
                let terminal = matches!(
                    item.status,
                    ark_contract::item::ItemStatus::Completed
                        | ark_contract::item::ItemStatus::Failed
                );
                if !terminal {
                    continue;
                }
                let Some(payload) = tool_payload(item) else {
                    continue;
                };
                attach_tool_call(
                    &mut history,
                    ToolCallRef {
                        call_id: payload.call_id.clone(),
                        name: payload.tool.clone(),
                        arguments: payload.arguments.clone().unwrap_or_default(),
                    },
                );
                let call_id = payload.call_id.clone();
                let tool = payload.tool.clone();
                history.push(ModelMessage {
                    role: ModelRole::Tool,
                    content: truncate_utf8(payload.output.as_deref().unwrap_or("")),
                    tool_call_id: Some(call_id),
                    name: Some(tool),
                    tool_calls: Vec::new(),
                });
            }
            _ => {}
        }
    }
    history
}

fn nonempty(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.trim().is_empty())
}

fn tool_payload(item: &ark_contract::ThreadItem) -> Option<&ark_contract::item::ToolPayload> {
    item.tool.as_ref()
}

/// Fold a replayed call into the last assistant row, or open a new one.
/// Ordering of user/assistant rows is never rearranged.
fn attach_tool_call(history: &mut Vec<ModelMessage>, call: ToolCallRef) {
    match history.last_mut() {
        Some(ModelMessage {
            role: ModelRole::Assistant,
            tool_calls,
            ..
        }) => tool_calls.push(call),
        _ => history.push(ModelMessage::assistant_with_tool_calls("", vec![call])),
    }
}

/// Tool outputs are context, not identity: cap them at 16 KiB on a UTF-8
/// boundary so a mid-codepoint cut cannot produce invalid text.
fn truncate_utf8(text: &str) -> String {
    const MAX: usize = 16 * 1024;
    if text.len() <= MAX {
        return text.to_string();
    }
    let mut end = MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// Adapter from `ark-tool`'s runner to `ark-agent`'s `ToolRunner` trait.
struct ToolRunnerAdapter {
    inner: Arc<RegistryToolRunner>,
    workspace: std::path::PathBuf,
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
                        self.workspace.clone(),
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
                        self.workspace.clone(),
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

    #[test]
    fn conversation_history_replays_context_checkpoints_as_system_messages() {
        let checkpoint =
            ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::Notice)
                .with_text("[context checkpoint]\nGoal: preserve state")
                .completed();

        assert_eq!(
            conversation_history(&[checkpoint]),
            vec![ModelMessage::system(
                "[context checkpoint]\nGoal: preserve state"
            )]
        );
    }

    /// A folded tool item as the store projects it: one `ToolCall` entry whose
    /// payload embeds the result (see `apply_tool_finished`).
    fn tool_item(
        call_id: &str,
        tool: &str,
        output: Option<&str>,
        status: ark_contract::item::ItemStatus,
    ) -> ark_contract::ThreadItem {
        let mut item =
            ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::ToolCall);
        item.status = status;
        item.tool = Some(ark_contract::item::ToolPayload {
            call_id: call_id.into(),
            tool: tool.into(),
            arguments: Some(serde_json::json!({"path": "a.txt"})),
            output: output.map(Into::into),
            ok: Some(status == ark_contract::item::ItemStatus::Completed),
            duration_ms: None,
        });
        item
    }

    #[test]
    fn paired_tool_rows_fold_into_the_assistant_history() {
        let items = vec![
            ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::UserMessage)
                .with_text("hi")
                .completed(),
            ark_contract::ThreadItem::new(
                TurnId::new(),
                ark_contract::item::ItemKind::AssistantMessage,
            )
            .with_text("calling")
            .completed(),
            tool_item(
                "c1",
                "read",
                Some("file body"),
                ark_contract::item::ItemStatus::Completed,
            ),
        ];
        assert_eq!(
            conversation_history(&items),
            vec![
                ModelMessage::user("hi"),
                ModelMessage::assistant_with_tool_calls(
                    "calling",
                    vec![ark_ports::ToolCallRef {
                        call_id: "c1".into(),
                        name: "read".into(),
                        arguments: serde_json::json!({"path": "a.txt"}),
                    }],
                ),
                ModelMessage {
                    role: ModelRole::Tool,
                    content: "file body".into(),
                    tool_call_id: Some("c1".into()),
                    name: Some("read".into()),
                    tool_calls: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn non_terminal_tool_items_are_not_replayed() {
        let streaming = tool_item(
            "c1",
            "read",
            None,
            ark_contract::item::ItemStatus::Streaming,
        );
        assert_eq!(conversation_history(&[streaming]), Vec::new());
    }

    #[test]
    fn failed_tool_results_are_replayed_with_empty_output_fallback() {
        let failed = tool_item("c1", "bash", None, ark_contract::item::ItemStatus::Failed);
        let history = conversation_history(&[failed]);
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].content, "");
        assert_eq!(history[1].tool_call_id.as_deref(), Some("c1"));
    }

    #[test]
    fn oversized_tool_output_is_truncated_at_16kib_bytes() {
        let big = "x".repeat(20 * 1024);
        let items = vec![tool_item(
            "c1",
            "read",
            Some(&big),
            ark_contract::item::ItemStatus::Completed,
        )];
        let history = conversation_history(&items);
        assert_eq!(history[1].content.len(), 16 * 1024);
    }

    #[test]
    fn tool_history_keeps_the_time_order_of_items() {
        let items = vec![
            ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::UserMessage)
                .with_text("q1")
                .completed(),
            tool_item(
                "c1",
                "read",
                Some("r1"),
                ark_contract::item::ItemStatus::Completed,
            ),
            ark_contract::ThreadItem::new(
                TurnId::new(),
                ark_contract::item::ItemKind::AssistantMessage,
            )
            .with_text("answer")
            .completed(),
            ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::UserMessage)
                .with_text("q2")
                .completed(),
        ];
        let history = conversation_history(&items);
        let roles: Vec<ModelRole> = history.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![
                ModelRole::User,
                ModelRole::Assistant,
                ModelRole::Tool,
                ModelRole::Assistant,
                ModelRole::User
            ]
        );
        // The tool call has no leading assistant text, so it lands on a
        // synthetic empty-content assistant row before its result.
        assert_eq!(history[1].content, "");
        assert_eq!(history[2].content, "r1");
        assert_eq!(history[3].content, "answer");
    }

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

        let end =
            finish_parent_turn(TurnOutcome::Completed, &store, &store, thread, turn, &inbox).await;

        assert!(matches!(end, TurnEnd::Completed));
        assert!(completion_inbox(&store, thread)
            .await
            .unwrap()
            .notification_ids()
            .is_empty());
    }

    #[tokio::test]
    async fn completed_parent_turn_reports_another_bounded_inbox_page() {
        let data_dir = TestDataDir::new();
        let store = FileSessionStore::open(StoreConfig::new(&data_dir.0)).unwrap();
        let thread = store.create_thread("parent").await.unwrap().thread_id;
        for index in 0..9 {
            store
                .append_event(
                    thread,
                    RuntimeEvent::ChildCompleted {
                        seq: 0,
                        thread_id: thread,
                        timestamp: Utc::now(),
                        notification_id: format!("child:{index}"),
                        child_id: ChildId::new(),
                        label: format!("worker {index}"),
                        status: "completed".to_string(),
                        summary: Some("done".to_string()),
                        error: None,
                        usage: None,
                        tool_invocations: None,
                    },
                )
                .await
                .unwrap();
        }

        let first = completion_inbox(&store, thread).await.unwrap();
        assert_eq!(first.notification_ids().len(), 8);
        let first_end = finish_parent_turn(
            TurnOutcome::Completed,
            &store,
            &store,
            thread,
            TurnId::new(),
            &first,
        )
        .await;
        assert!(matches!(
            first_end,
            TurnEnd::CompletedWithPendingCompletions
        ));

        let second = completion_inbox(&store, thread).await.unwrap();
        assert_eq!(second.notification_ids().len(), 1);
        let second_end = finish_parent_turn(
            TurnOutcome::Completed,
            &store,
            &store,
            thread,
            TurnId::new(),
            &second,
        )
        .await;
        assert!(matches!(second_end, TurnEnd::Completed));
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
                model_request_failed: false,
            },
            &store,
            &store,
            thread,
            TurnId::new(),
            &inbox,
        )
        .await;

        assert!(matches!(end, TurnEnd::Failed { message, .. } if message == "model failed"));
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
        let config = RuntimeConfig {
            tuning: ark_config::TuningConfig {
                context_limit_tokens: 100_000,
                ..ark_config::TuningConfig::default()
            },
            ..RuntimeConfig::default()
        };
        let mut provider = ProviderConfig::new("p", "m");
        provider.context_window_tokens = Some(32_000);
        provider.max_tokens = Some(4_096);

        let limits = AgentTurnRunner::limits_for_turn(&config, Some(&provider));

        assert_eq!(limits.context_limit_tokens, 27_904);
    }

    #[tokio::test]
    async fn startup_aborts_a_parent_turn_left_open_by_a_previous_process() {
        let data_dir = TestDataDir::new();
        let store = FileSessionStore::open(StoreConfig::new(&data_dir.0)).unwrap();
        let thread = store.create_thread("parent").await.unwrap().thread_id;
        let turn = TurnId::new();
        store
            .append_event(
                thread,
                RuntimeEvent::TurnStarted {
                    seq: 0,
                    thread_id: thread,
                    turn_id: turn,
                    timestamp: Utc::now(),
                    auto_continuation: false,
                },
            )
            .await
            .unwrap();

        assert_eq!(abort_interrupted_turns(&store).await.unwrap(), 1);
        assert_eq!(abort_interrupted_turns(&store).await.unwrap(), 0);

        let events = store.replay_interaction_events(thread).await.unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::TurnAborted { turn_id, reason, .. }
                if *turn_id == turn && reason == "runtime restarted before turn could finish"
        )));
    }

    #[test]
    fn conversation_history_keeps_visible_messages_and_excludes_reasoning() {
        let turn = TurnId::new();
        let items = vec![
            ark_contract::ThreadItem::new(turn, ark_contract::item::ItemKind::UserMessage)
                .with_text("first question"),
            ark_contract::ThreadItem::new(turn, ark_contract::item::ItemKind::Reasoning)
                .with_text("private reasoning"),
            ark_contract::ThreadItem::new(turn, ark_contract::item::ItemKind::AssistantMessage)
                .with_text("first answer"),
        ];

        let history = conversation_history(&items);

        assert_eq!(history.len(), 2);
        assert_eq!(history[0], ModelMessage::user("first question"));
        assert_eq!(history[1], ModelMessage::assistant("first answer"));
    }

    #[test]
    fn request_user_input_is_registered_for_both_agent_tool_sets() {
        let skills = Arc::new(ark_skills::SkillRuntime::new(
            ark_skills::SkillsConfig::default(),
            std::env::temp_dir(),
        ));
        let child_tools = build_child_tools(Arc::clone(&skills)).expect("child tools build");
        assert!(child_tools
            .names()
            .contains(&"request_user_input".to_string()));

        // The parent set adds `delegate_task` on top of the child set.
        let parent_tools =
            build_tools(Arc::new(TestSubagentRunner), Arc::clone(&skills)).expect("tools build");
        let names = parent_tools.names();
        assert!(names.contains(&"request_user_input".to_string()));
        assert!(names.contains(&"bash".to_string()));
        assert!(names.contains(&"delegate_task".to_string()));

        // The wire schema fixes the prompt contract: {prompt: string, required}.
        let schema = parent_tools
            .schemas()
            .into_iter()
            .find(|schema| schema["name"] == "request_user_input")
            .expect("request_user_input schema present");
        assert_eq!(schema["parameters"]["required"][0], "prompt");
        assert_eq!(
            schema["parameters"]["properties"]["prompt"]["type"],
            "string"
        );
    }

    /// Registry-shape stub: `build_tools` only needs the runner for the
    /// `delegate_task` tool; this test never runs a child.
    struct TestSubagentRunner;

    #[async_trait::async_trait]
    impl ark_ports::SubagentRunner for TestSubagentRunner {
        fn is_enabled(&self) -> bool {
            true
        }

        fn active_count(&self) -> u32 {
            0
        }

        async fn delegate(
            &self,
            _request: ark_ports::SubagentRequest,
        ) -> ark_common::Result<ark_ports::SubagentOutcome> {
            Err(ark_common::ArkError::internal("not used by this test"))
        }

        async fn delegate_background(
            &self,
            _request: ark_ports::SubagentRequest,
        ) -> ark_common::Result<ark_common::ChildId> {
            Err(ark_common::ArkError::internal("not used by this test"))
        }

        async fn prepare_child(
            &self,
            _request: ark_ports::SubagentRequest,
        ) -> ark_common::Result<ark_common::ChildId> {
            Err(ark_common::ArkError::internal("not used by this test"))
        }

        async fn run_child(
            &self,
            _child: ark_common::ChildId,
        ) -> ark_common::Result<ark_ports::SubagentOutcome> {
            Err(ark_common::ArkError::internal("not used by this test"))
        }
    }
}
