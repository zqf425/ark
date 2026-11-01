//! Child-run execution: the *same* reactor, driven against a child session.
//!
//! Design §7.8.0 keeps exactly one decision loop in the runtime. A sub-agent is
//! therefore not a second agent: it is another `Reactor::run()` over a different
//! thread id, with its own event log (`threads/<parent>/children/<child>/`), its
//! own cancel token and its own steering queue.
//!
//! # Why the bridges exist
//!
//! `ark-delegation` hands the executor its control handles as `dyn CancelHandle`
//! / `dyn SteerHandle`, because that crate may not depend on `ark-agent`. The
//! reactor, in turn, needs the concrete `CancelToken` / `SteerQueue` it awaits and
//! drains. This module is the only place both shapes meet, so the two adapters
//! live here rather than leaking either type across the boundary.
//!
//! # What it deliberately does not do
//!
//! It never writes a run record, never decides whether a spawn is allowed, and
//! never reports usage it did not observe. Budget, records and activity events
//! belong to `DelegationRuntime`; this type only runs the turn and reports what
//! actually happened.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ark_agent::{
    EventSinkWriter, ModelSeed, Reactor, ReactorConfig, ToolCallRequest, TurnEvents, TurnLimits,
    TurnMachine, TurnOutcome,
};
use ark_common::{Result, ThreadId, TurnId, Usage};
use ark_config::RuntimeConfig;
use ark_contract::{item::ItemKind, RuntimeEvent};
use ark_delegation::{ChildTurnContext, ChildTurnExecutor, ChildTurnReport, ChildTurnStatus};
use ark_obs::UsageRecorder;
use ark_ports::{
    CancelHandle, EventSink, ModelClient, SandboxPolicy, SessionStore, SteerHandle, ToolOutput,
    ToolRegistryPort,
};
use ark_tool::{RegistryToolRunner, ToolRegistry};

use crate::model;
use crate::workspace::WorkspaceRoots;

/// How often the bridges look for a signal from the delegation runtime.
///
/// Abort and steering are human-paced actions, so a 50 ms worst-case latency is
/// invisible while keeping both bridges lock-free and allocation-free on the hot
/// path. Neither loop runs a model round, so they cannot stall a turn.
const BRIDGE_TICK: Duration = Duration::from_millis(50);

/// Resolve a child override against its parent setting. A `None` child value is
/// intentionally not a reset: only an explicit value changes inherited state.
fn child_setting<T>(child: Option<T>, parent: Option<T>) -> Option<T> {
    child.or(parent)
}

/// Owns bridge tasks so panic unwinding cannot detach permanent polling loops.
struct BridgeTasks {
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl BridgeTasks {
    fn new(handles: Vec<tokio::task::JoinHandle<()>>) -> Self {
        Self { handles }
    }

    async fn shutdown(mut self) {
        for handle in self.handles.drain(..) {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl Drop for BridgeTasks {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

/// Runs one child turn with the real reactor.
pub struct ChildTurnExecutorImpl {
    config: Arc<ark_config::ConfigStore>,
    providers: Arc<ark_provider::ProviderStore>,
    usage: Arc<UsageRecorder>,
    /// The parent turn's client, reused when the child did not ask for another
    /// provider. Reusing it means a child costs no extra configuration.
    parent_model: Arc<dyn ModelClient>,
    /// Child tool runner, built over a registry that **excludes** `delegate_task`,
    /// which is what enforces `max_depth = 1` structurally instead of relying on
    /// the depth check alone.
    tools: Arc<RegistryToolRunner>,
    tool_schemas: Vec<serde_json::Value>,
    sink: Arc<dyn EventSink>,
    store: Arc<dyn SessionStore>,
    roots: Arc<WorkspaceRoots>,
    skills: Arc<ark_skills::SkillRuntime>,
}

impl ChildTurnExecutorImpl {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Arc<ark_config::ConfigStore>,
        providers: Arc<ark_provider::ProviderStore>,
        usage: Arc<UsageRecorder>,
        parent_model: Arc<dyn ModelClient>,
        child_tools: Arc<ToolRegistry>,
        sink: Arc<dyn EventSink>,
        store: Arc<dyn SessionStore>,
        policy: Arc<dyn SandboxPolicy>,
        roots: Arc<WorkspaceRoots>,
        skills: Arc<ark_skills::SkillRuntime>,
    ) -> Self {
        let tool_schemas = child_tools.schemas();
        let runner = Arc::new(RegistryToolRunner::new(
            child_tools,
            Arc::clone(&policy),
            roots.current(),
        ));
        Self {
            config,
            providers,
            usage,
            parent_model,
            tools: runner,
            tool_schemas,
            sink,
            store,
            roots,
            skills,
        }
    }

    /// Turn limits for a child run, read from the live config snapshot.
    ///
    /// A child gets the same budget as a parent turn unless the request asked
    /// for a narrower one. Copying the values here (instead of hardcoding
    /// defaults) is what makes a Settings change apply to the next child without
    /// a restart, which is the `Hot` contract for `tuning`.
    fn limits_of(
        snapshot: &RuntimeConfig,
        provider: Option<&ark_llm::ProviderConfig>,
    ) -> TurnLimits {
        let tuning = &snapshot.tuning;
        // A child is a *bounded* sub-task: it gets the same ceilings, never more.
        TurnLimits {
            max_rounds: tuning.max_rounds.max(1),
            max_tokens: tuning.max_tokens.max(1),
            max_tool_calls: tuning.max_tool_calls.max(1),
            max_identical_calls: tuning.max_identical_calls.max(1),
            context_limit_tokens: crate::model::context_limit_tokens(
                tuning.context_limit_tokens,
                provider,
            ),
            keep_recent_messages: tuning.keep_recent_messages.max(1),
            max_compactions: TurnLimits::default().max_compactions,
        }
    }

    /// Resolve this child's provider from the live table before using its parent
    /// fallback. That gives children the same hot-update semantics as parents.
    fn provider_for(
        &self,
        provider_id: Option<&str>,
        model: Option<&str>,
    ) -> Result<Option<ark_llm::ProviderConfig>> {
        let provider_id = provider_id.filter(|id| !id.trim().is_empty());
        let mut provider = match provider_id {
            Some(id) => Some(self.providers.resolve(Some(id)).ok_or_else(|| {
                ark_common::ArkError::validation(format!("child provider is unavailable: {id}"))
            })?),
            None => self.providers.resolve(None),
        };
        if let (Some(provider), Some(model)) = (
            &mut provider,
            model.filter(|model| !model.trim().is_empty()),
        ) {
            if model != provider.model {
                // Catalog metadata belongs to the stored model id. A child
                // override has no associated catalog response, so retaining
                // the old window would incorrectly compact against another
                // model's limit.
                provider.model = model.to_string();
                provider.context_window_tokens = None;
            }
        }
        Ok(provider)
    }

    /// Build the transport client for a resolved provider, or retain the local
    /// parent fallback when no usable provider is configured.
    fn model_for(&self, provider: Option<ark_llm::ProviderConfig>) -> Arc<dyn ModelClient> {
        match provider {
            Some(provider) => model::select_from_config(provider, Arc::clone(&self.usage)).0,
            None => Arc::clone(&self.parent_model),
        }
    }
}

#[async_trait::async_trait]
impl ChildTurnExecutor for ChildTurnExecutorImpl {
    async fn run_child_turn(&self, ctx: ChildTurnContext) -> Result<ChildTurnReport> {
        // A child is addressed by its own id; the store resolves that id to the
        // inlined `threads/<parent>/children/<child>/` directory.
        let thread_id = ThreadId::from_uuid(ctx.child_id.as_uuid());
        let turn_id = TurnId::new();
        let events = TurnEvents::new(thread_id, turn_id);
        let writer = Arc::new(EventSinkWriter::new(Arc::clone(&self.sink), events.clone()));

        let cancel = ark_agent::CancelToken::new();
        let cancel_bridge = spawn_cancel_bridge(Arc::clone(&ctx.cancel), cancel.clone());
        let steer = Arc::new(ark_agent::exec::steer::SteerQueue::new());
        let steer_bridge = spawn_steer_bridge(Arc::clone(&ctx.steer), Arc::clone(&steer));
        let bridges = BridgeTasks::new(vec![cancel_bridge, steer_bridge]);

        // Child defaults are the parent thread's effective runtime settings.
        // Explicit child values win, and process defaults are used only when the
        // parent has no override. This keeps a delegated task in the same
        // workspace, permission envelope and model unless it deliberately asks
        // to narrow or replace them.
        let parent = self.store.get_thread(ctx.parent_thread_id).await?;
        let parent_runtime = parent.runtime;
        let workspace = child_setting(
            ctx.workspace.clone(),
            parent_runtime.workspace.clone().map(PathBuf::from),
        )
        .unwrap_or_else(|| self.roots.current());
        if !workspace.is_dir() {
            return Err(ark_common::ArkError::validation(format!(
                "child workspace does not exist: {}",
                workspace.display()
            )));
        }
        let workspace = workspace
            .canonicalize()
            .map_err(|_| ark_common::ArkError::validation("cannot canonicalize child workspace"))?;
        let requested_provider = ctx
            .provider_id
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        let provider_id = child_setting(requested_provider, parent_runtime.provider_id.as_deref());
        // A different provider owns its own default model. Only inherit a parent
        // model when the child retains the parent's provider selection.
        let model_override = if requested_provider.is_some() {
            ctx.model
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        } else {
            child_setting(
                ctx.model
                    .as_deref()
                    .filter(|value| !value.trim().is_empty()),
                parent_runtime.model.as_deref(),
            )
        };
        let config = self.config.snapshot();
        let parent_autonomy = crate::wiring::autonomy_from_override_or_default(
            parent_runtime.autonomy.as_deref(),
            &config,
        )?;
        let autonomy = match ctx
            .autonomy
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            Some(value) => {
                let requested = crate::wiring::autonomy_from_override(Some(value))?;
                if autonomy_rank(requested) > autonomy_rank(parent_autonomy) {
                    return Err(ark_common::ArkError::validation(
                        "child autonomy cannot exceed the parent thread",
                    ));
                }
                requested
            }
            None => parent_autonomy,
        };
        let sandbox: Arc<dyn SandboxPolicy> =
            Arc::new(ark_tool::BuiltinSandboxPolicy::new(autonomy));
        let tools: Arc<dyn ark_agent::ToolRunner> = Arc::new(ChildToolRunner {
            inner: Arc::clone(&self.tools),
            workspace: workspace.clone(),
            thread_id,
            turn_id,
        });

        // Same static prefix as a parent turn: byte-identical bytes are what let
        // the provider cache the prefix across parent and child.
        let mut seed_messages = vec![
            ark_ports::ModelMessage::system(crate::wiring::SYSTEM_PROMPT),
            ark_ports::ModelMessage::system(format!("workspace root: {}", workspace.display())),
            // A child is told what it is: it must answer, not converse.
            ark_ports::ModelMessage::system(
                "You are a child run. Finish the delegated task and answer with the result. \
                 You cannot delegate further.",
            ),
        ];
        // Skill instructions ride the dynamic tail, exactly like a parent turn.
        let resolution = self.skills.resolve_for_turn(&ctx.prompt);
        for instruction in &resolution.instructions {
            seed_messages.push(ark_ports::ModelMessage::system(format!(
                "[skill instructions]\n{instruction}"
            )));
        }

        let snapshot = self.config.snapshot();
        let provider = self.provider_for(provider_id, model_override)?;
        let mut reactor = Reactor::new(ReactorConfig {
            model: self.model_for(provider.clone()),
            tools,
            writer,
            gates: Arc::new(ark_agent::AllowAllGates),
            sandbox,
            events,
            cancel: cancel.clone(),
            seed: ModelSeed {
                messages: seed_messages,
                tools: self.tool_schemas.clone(),
                temperature: provider.as_ref().and_then(|provider| provider.temperature),
                max_tokens: provider.as_ref().and_then(|provider| provider.max_tokens),
            },
            machine: TurnMachine::new(Self::limits_of(&snapshot, provider.as_ref())),
            chunk_buffer: crate::wiring::CHUNK_BUFFER,
        })
        .with_steering(steer);
        reactor.push_user(ctx.prompt.clone());
        let outcome = reactor.run().await;

        // Both adapters otherwise poll forever after a normally completed or
        // failed reactor run. Aborting and awaiting them leaves no child task
        // behind before report construction performs any further async work.
        bridges.shutdown().await;

        // Real counters only: the reactor folds every provider sample into its
        // machine, so the report cannot invent tokens.
        let progress = reactor.machine().progress();
        let (usage, tool_invocations) = self
            .observed_counts(thread_id, progress.tokens_used, progress.tool_calls)
            .await;

        Ok(match outcome {
            Ok(TurnOutcome::Completed) => {
                let summary = self.final_answer(thread_id).await;
                ChildTurnReport {
                    status: ChildTurnStatus::Completed,
                    summary,
                    usage,
                    tool_invocations,
                }
            }
            Ok(TurnOutcome::Aborted { reason }) => ChildTurnReport {
                status: ChildTurnStatus::Aborted { reason },
                summary: self.final_answer(thread_id).await,
                usage,
                tool_invocations,
            },
            Ok(TurnOutcome::Failed { message, .. }) => ChildTurnReport {
                status: ChildTurnStatus::Failed { message },
                summary: String::new(),
                usage,
                tool_invocations,
            },
            Err(err) => ChildTurnReport {
                status: ChildTurnStatus::Failed {
                    message: err.message,
                },
                summary: String::new(),
                usage,
                tool_invocations,
            },
        })
    }
}

fn autonomy_rank(value: ark_tool::Autonomy) -> u8 {
    match value {
        ark_tool::Autonomy::Strict => 0,
        ark_tool::Autonomy::WorkspaceWrite => 1,
        ark_tool::Autonomy::Unattended => 2,
    }
}

impl ChildTurnExecutorImpl {
    /// Usage and tool counts, taken from what the child actually recorded.
    ///
    /// The split prompt/completion counters live in the child's own `usage`
    /// events, so they are replayed rather than reconstructed. When no sample
    /// exists the fallback reports only the total the machine counted, leaving
    /// the split at zero instead of guessing it.
    async fn observed_counts(
        &self,
        thread_id: ThreadId,
        fallback_tokens: u64,
        fallback_tools: u32,
    ) -> (Usage, u64) {
        let events = self
            .store
            .replay_events(thread_id, 0)
            .await
            .unwrap_or_default();
        let mut usage = Usage::default();
        let mut tools = 0u64;
        let mut saw_usage = false;
        for event in &events {
            match event {
                RuntimeEvent::Usage { usage: sample, .. } => {
                    usage.merge(sample);
                    saw_usage = true;
                }
                RuntimeEvent::ToolCallStarted { .. } => tools += 1,
                _ => {}
            }
        }
        if !saw_usage {
            // No provider sample reached the log: report the machine's total and
            // leave the split unknown rather than splitting it by assumption.
            usage.total_tokens = fallback_tokens;
        }
        let tool_invocations = if tools > 0 {
            tools
        } else {
            fallback_tools as u64
        };
        (usage, tool_invocations)
    }

    /// The child's last assistant message, which is what the parent reads.
    async fn final_answer(&self, thread_id: ThreadId) -> String {
        let items = match self.store.read_items(thread_id).await {
            Ok(items) => items,
            Err(_) => return String::new(),
        };
        items
            .iter()
            .rev()
            .find(|item| item.kind == ItemKind::AssistantMessage)
            .and_then(|item| item.text.clone())
            .unwrap_or_default()
    }
}

/// Adapter from `ark-tool`'s runner to the reactor's `ToolRunner`.
struct ChildToolRunner {
    inner: Arc<RegistryToolRunner>,
    workspace: PathBuf,
    thread_id: ThreadId,
    turn_id: TurnId,
}

#[async_trait::async_trait]
impl ark_agent::ToolRunner for ChildToolRunner {
    async fn execute(&self, call: &ToolCallRequest) -> Result<ToolOutput> {
        // Same rule as the parent path: consume the decision the reactor already
        // made for this call instead of asking the policy a second time
        // (design §7.3, P1-5).
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

/// Forward `cancel` from the delegation runtime into the reactor's token.
fn spawn_cancel_bridge(
    source: Arc<dyn CancelHandle>,
    target: ark_agent::CancelToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if target.is_cancelled() {
                return;
            }
            if source.is_cancelled() {
                target.cancel();
                return;
            }
            tokio::time::sleep(BRIDGE_TICK).await;
        }
    })
}

/// Forward queued corrections from the delegation runtime into the reactor's
/// queue. Ordering is preserved because the source hands them out in arrival
/// order and this loop pushes them in that same order.
fn spawn_steer_bridge(
    source: Arc<dyn SteerHandle>,
    target: Arc<ark_agent::exec::steer::SteerQueue>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            for text in source.drain() {
                // A full target queue drops the correction; the runtime already
                // bounded the source with the same capacity, so this only happens
                // if the reactor is behind by a full queue.
                if !target.push(text) {
                    tracing::warn!("child steering queue full; correction dropped");
                }
            }
            tokio::time::sleep(BRIDGE_TICK).await;
        }
    })
}

/// Live sub-agent policy, read from the config snapshot on every spawn.
///
/// `subagents` is a `Hot` field (design §7.15), so a Settings change must apply
/// to the *next* child without a restart. Holding the store instead of a copied
/// value is what makes that true.
pub struct ConfigSubagentPolicy {
    config: Arc<ark_config::ConfigStore>,
}

impl ConfigSubagentPolicy {
    pub fn new(config: Arc<ark_config::ConfigStore>) -> Self {
        Self { config }
    }
}

impl ark_ports::SubagentPolicySource for ConfigSubagentPolicy {
    fn policy(&self) -> ark_ports::SubagentPolicy {
        let settings: RuntimeConfig = (*self.config.snapshot()).clone();
        ark_ports::SubagentPolicy {
            enabled: settings.subagents.enabled,
            max_concurrency: settings.subagents.max_concurrency.max(1),
            max_queued: DEFAULT_MAX_QUEUED,
        }
    }
}

/// How many children may *wait* for a slot.
///
/// Bounded so a parent turn that spawns in a loop fails fast instead of
/// accumulating unbounded work it will never await.
pub const DEFAULT_MAX_QUEUED: u32 = 20;

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use super::*;
    use ark_ports::SubagentPolicySource;

    #[test]
    fn child_runtime_override_wins_and_absence_inherits_parent() {
        assert_eq!(
            child_setting(Some("strict".to_string()), Some("unattended".to_string())),
            Some("strict".to_string())
        );
        assert_eq!(
            child_setting::<String>(None, Some("parent-model".to_string())),
            Some("parent-model".to_string())
        );
        assert_eq!(
            child_setting(
                Some("".to_string()).filter(|value| !value.trim().is_empty()),
                Some("workspace_write".to_string()),
            ),
            Some("workspace_write".to_string())
        );
    }

    #[derive(Default)]
    struct TestCancelHandle {
        cancelled: AtomicBool,
    }

    impl CancelHandle for TestCancelHandle {
        fn cancel(&self) {
            self.cancelled.store(true, Ordering::Release);
        }

        fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::Acquire)
        }
    }

    #[derive(Default)]
    struct TestSteerHandle {
        pending: Mutex<VecDeque<String>>,
    }

    impl SteerHandle for TestSteerHandle {
        fn push(&self, text: &str) -> bool {
            self.pending.lock().unwrap().push_back(text.to_string());
            true
        }

        fn len(&self) -> usize {
            self.pending.lock().unwrap().len()
        }

        fn drain(&self) -> Vec<String> {
            self.pending.lock().unwrap().drain(..).collect()
        }
    }

    fn policy_for(enabled: bool, max_concurrency: u32) -> ark_ports::SubagentPolicy {
        let config = RuntimeConfig {
            subagents: ark_contract::SubagentSettings {
                enabled,
                max_concurrency,
                auto_continue: false,
            },
            ..RuntimeConfig::default()
        };
        let store = Arc::new(ark_config::ConfigStore::new(config));
        ConfigSubagentPolicy::new(store).policy()
    }

    #[tokio::test]
    async fn cancel_bridge_forwards_runtime_cancel_and_can_be_joined() {
        let source = Arc::new(TestCancelHandle::default());
        let target = ark_agent::CancelToken::new();
        let bridge = spawn_cancel_bridge(source.clone(), target.clone());

        source.cancel();
        tokio::time::timeout(BRIDGE_TICK * 3, async {
            while !target.is_cancelled() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("runtime cancellation should reach the reactor");

        bridge
            .await
            .expect("cancel bridge should exit after forwarding");
    }

    #[tokio::test]
    async fn steer_bridge_drains_in_order_and_can_be_stopped() {
        let source = Arc::new(TestSteerHandle::default());
        source.push("first");
        source.push("second");
        let target = Arc::new(ark_agent::exec::steer::SteerQueue::new());
        let bridge = spawn_steer_bridge(source, Arc::clone(&target));

        tokio::time::timeout(BRIDGE_TICK * 3, async {
            while target.len() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("steering should reach the reactor queue");
        assert_eq!(target.drain(), vec!["first", "second"]);

        bridge.abort();
        assert!(bridge.await.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn dropping_bridge_tasks_aborts_pollers_during_unwind() {
        struct DropProbe(Arc<AtomicBool>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let probe = DropProbe(Arc::clone(&dropped));
        let handle = tokio::spawn(async move {
            let _probe = probe;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;

        drop(BridgeTasks::new(vec![handle]));
        tokio::time::timeout(BRIDGE_TICK * 3, async {
            while !dropped.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropping the RAII owner must abort and drop every bridge future");
    }

    #[test]
    fn policy_is_read_live_so_settings_apply_without_a_restart() {
        assert!(!policy_for(false, 2).enabled);
        assert!(policy_for(true, 2).enabled);
        assert_eq!(policy_for(true, 3).max_concurrency, 3);
    }

    #[test]
    fn a_zero_concurrency_cap_becomes_one_instead_of_deadlocking() {
        // Clamped at the config layer too, but the runtime must never build a
        // gate that can never be acquired.
        assert_eq!(policy_for(true, 0).max_concurrency, 1);
    }
}
