//! Child-run execution: the *same* reactor, driven against a child session.
//!
//! Design §7.8.0 keeps exactly one decision loop in the runtime. A sub-agent is
//! therefore not a second agent: it is another `Reactor::run()` over a different
//! thread id, with its own event log (`threads/<parent>/children/<child>/`), its
//! own cancel token and its own steering queue.
//!
//! # What it deliberately does not do
//!
//! The delegation runtime hands the executor the same concrete
//! `ark-ports::CancelToken` / `SteerQueue` the reactor awaits and drains, so no
//! adapter or polling bridge is needed between the two shapes.
//!
//! It never writes a run record, never decides whether a spawn is allowed, and
//! never reports usage it did not observe. Budget, records and activity events
//! belong to `DelegationRuntime`; this type only runs the turn and reports what
//! actually happened.

use std::path::PathBuf;
use std::sync::Arc;

use ark_agent::{EventSinkWriter, ModelSeed, TurnEvents, TurnMachine, TurnOutcome};
use ark_common::{Result, ThreadId, TurnId, Usage};
use ark_config::RuntimeConfig;
use ark_contract::{item::ItemKind, RuntimeEvent};
use ark_delegation::{ChildTurnContext, ChildTurnExecutor, ChildTurnReport, ChildTurnStatus};
use ark_obs::UsageRecorder;
use ark_ports::{EventSink, ModelClient, SandboxPolicy, SessionStore, ToolRegistryPort};
use ark_tool::{RegistryToolRunner, ToolRegistry};

use crate::model;
use crate::wiring::turn_common::{
    build_reactor, chunk_buffer_for_turn, concurrency_table, limits_for_turn,
    push_project_instructions, resolve_workspace, seed_prefix, skill_tail, HookToolRunner,
    HookedToolRunner, ReactorSpec, CHILD_ROLE_LINE,
};
use crate::workspace::WorkspaceRoots;

/// Resolve a child override against its parent setting. A `None` child value is
/// intentionally not a reset: only an explicit value changes inherited state.
fn child_setting<T>(child: Option<T>, parent: Option<T>) -> Option<T> {
    child.or(parent)
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
    /// Kept so the catalogue is evaluated **per turn** (requirements/
    /// plugin-hook-bus.md, P1): a static copy captured at construction would
    /// bypass the P2 `tool_catalog.filter` hook for every child run.
    child_tools: Arc<ToolRegistry>,
    /// Hook bus shared with the parent runner and AppState.
    hooks: Arc<dyn ark_ports::HookBroker>,
    sink: Arc<dyn EventSink>,
    store: Arc<dyn SessionStore>,
    roots: Arc<WorkspaceRoots>,
    skills: Arc<ark_skills::SkillRuntime>,
}

/// Effective child settings resolved from the parent thread plus overrides.
struct ChildTurnSettings {
    workspace: PathBuf,
    provider_id: Option<String>,
    model_override: Option<String>,
    autonomy: ark_tool::Autonomy,
}

/// A different provider owns its own default model: only inherit a parent
/// model when the child retains the parent's provider selection.
fn child_model_override<'a>(
    requested: Option<&'a str>,
    provider_switched: bool,
    parent_model: Option<&'a str>,
) -> Option<&'a str> {
    let requested = requested.filter(|value| !value.trim().is_empty());
    if provider_switched {
        requested
    } else {
        child_setting(requested, parent_model)
    }
}

/// Resolve the child's autonomy: an explicit override may narrow but never
/// widen the parent thread's effective setting.
fn capped_child_autonomy(
    requested: Option<&str>,
    parent: ark_tool::Autonomy,
) -> Result<ark_tool::Autonomy> {
    match requested.filter(|value| !value.trim().is_empty()) {
        Some(value) => {
            let requested = crate::wiring::autonomy_from_override(Some(value))?;
            if requested.rank() > parent.rank() {
                return Err(ark_common::ArkError::validation(
                    "child autonomy cannot exceed the parent thread",
                ));
            }
            Ok(requested)
        }
        None => Ok(parent),
    }
}

/// Fold a reactor outcome into the child-run report the parent reads.
fn child_report(
    outcome: ark_common::Result<ark_agent::TurnOutcome>,
    summary: String,
    usage: Usage,
    tool_invocations: u64,
) -> ChildTurnReport {
    let status = match outcome {
        Ok(TurnOutcome::Completed) => ChildTurnStatus::Completed,
        Ok(TurnOutcome::Aborted { reason }) => ChildTurnStatus::Aborted { reason },
        Ok(TurnOutcome::Failed { message, .. }) => ChildTurnStatus::Failed { message },
        Err(err) => ChildTurnStatus::Failed {
            message: err.message,
        },
    };
    ChildTurnReport {
        status,
        summary,
        usage,
        tool_invocations,
    }
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
        hooks: Arc<dyn ark_ports::HookBroker>,
    ) -> Self {
        let runner = Arc::new(RegistryToolRunner::new(
            Arc::clone(&child_tools),
            Arc::clone(&policy),
            roots.current(),
        ));
        Self {
            config,
            providers,
            usage,
            parent_model,
            tools: runner,
            child_tools,
            hooks,
            sink,
            store,
            roots,
            skills,
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
        // `resolve_for_model` applies the inventory entry recorded for the
        // override model (context window); an override without one clears the
        // window, because the stored limit belongs to the stored model id.
        // It also rejects an override the provider's inventory does not list,
        // so a mismatched model/provider pair fails here with a diagnostic
        // that names the served models, not at the provider with a 404.
        let model = model
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        match self.providers.resolve_for_model(provider_id, model) {
            // "No default provider configured": fall through to the parent
            // fallback. Every other error (named provider missing, key
            // absent, override the inventory does not list) is a real
            // failure whose diagnostic the child run should surface.
            Ok(provider) => Ok(Some(provider)),
            Err(error)
                if provider_id.is_none() && error.code == ark_common::ArkErrorCode::NotFound =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Build the transport client for a resolved provider, or retain the local
    /// parent fallback when no usable provider is configured.
    fn model_for(&self, provider: Option<ark_llm::ProviderConfig>) -> Arc<dyn ModelClient> {
        match provider {
            Some(provider) => model::select_from_config(provider, Arc::clone(&self.usage)).0,
            None => Arc::clone(&self.parent_model),
        }
    }

    /// Assemble the child seed: the same static prefix as a parent turn (the
    /// shared builder guarantees byte-identical bytes, which is what lets the
    /// provider cache the prefix across parent and child), the child role line,
    /// the workspace's project instructions, then skill instructions. The
    /// catalog line does not ride a child seed: a child cannot load skills
    /// itself.
    fn child_seed(
        &self,
        workspace: &std::path::Path,
        prompt: &str,
    ) -> Vec<ark_ports::ModelMessage> {
        let mut seed_messages = seed_prefix(workspace);
        // A child is told what it is: it must answer, not converse.
        seed_messages.push(ark_ports::ModelMessage::system(CHILD_ROLE_LINE));
        push_project_instructions(workspace, &mut seed_messages);
        let resolution = self.skills.resolve_for_turn(prompt);
        seed_messages.extend(skill_tail(&resolution, false));
        seed_messages
    }

    /// Effective child settings: the parent thread's runtime values, narrowed
    /// by explicit child overrides (workspace, provider, model, autonomy).
    async fn inherited_settings(
        &self,
        ctx: &ChildTurnContext,
        snapshot: &RuntimeConfig,
    ) -> Result<ChildTurnSettings> {
        let parent = self.store.get_thread(ctx.parent_thread_id).await?;
        let parent_runtime = parent.runtime;
        let workspace = resolve_workspace(
            ctx.workspace.as_deref(),
            parent_runtime.workspace.clone().map(PathBuf::from),
            self.roots.current(),
            "child",
        )?;
        let requested_provider = ctx
            .provider_id
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        let provider_id = child_setting(
            requested_provider.map(ToString::to_string),
            parent_runtime.provider_id.clone(),
        );
        // A different provider owns its own default model: only inherit a
        // parent model when the child retains the parent's provider selection.
        let model_override = child_model_override(
            ctx.model.as_deref(),
            requested_provider.is_some(),
            parent_runtime.model.as_deref(),
        )
        .map(ToString::to_string);
        let parent_autonomy = crate::wiring::autonomy_from_override_or_default(
            parent_runtime.autonomy.as_deref(),
            snapshot,
        )?;
        Ok(ChildTurnSettings {
            workspace,
            provider_id,
            model_override,
            autonomy: capped_child_autonomy(ctx.autonomy.as_deref(), parent_autonomy)?,
        })
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

        let cancel = Arc::clone(&ctx.cancel);
        let steer = Arc::clone(&ctx.steer);

        // Child defaults are the parent thread's effective runtime settings.
        // Explicit child values win, and process defaults are used only when the
        // parent has no override. This keeps a delegated task in the same
        // workspace, permission envelope and model unless it deliberately asks
        // to narrow or replace them.
        // One snapshot per turn, same rule as the parent path: autonomy and
        // limits must come from the same config state. The rule table wraps the
        // child's (already capped) autonomy the same way it wraps the parent's.
        let snapshot = self.config.snapshot();
        let settings = self.inherited_settings(&ctx, &snapshot).await?;
        let workspace = settings.workspace;
        let sandbox: Arc<dyn SandboxPolicy> = Arc::new(ark_tool::ApprovalPolicy::new(
            ark_tool::BuiltinSandboxPolicy::new(settings.autonomy),
            snapshot.approvals.clone(),
        ));
        // Plugin decide first, then the shell `pre_tool_use` hook, then
        // dispatch — the same pinned order the parent path uses.
        let tools: Arc<dyn ark_agent::ToolRunner> = Arc::new(HookToolRunner::new(
            Arc::clone(&self.hooks),
            thread_id,
            turn_id,
            HookedToolRunner::new(
                Arc::clone(&self.tools),
                Arc::clone(&self.config),
                workspace.clone(),
                thread_id,
                turn_id,
            ),
        ));

        let seed_messages = self.child_seed(&workspace, &ctx.prompt);

        let snapshot = self.config.snapshot();
        let provider = self.provider_for(
            settings.provider_id.as_deref(),
            settings.model_override.as_deref(),
        )?;
        let model = model::hooked(
            Arc::clone(&self.hooks),
            thread_id,
            turn_id,
            self.model_for(provider.clone()),
        );
        let mut reactor = build_reactor(ReactorSpec {
            model,
            tools,
            writer,
            gates: Arc::new(ark_agent::AllowAllGates),
            sandbox,
            events,
            cancel: (*cancel).clone(),
            seed: ModelSeed {
                messages: seed_messages,
                // Evaluated per turn so a tool-catalog hook (P2) always sees the
                // current registry; the parent path already does exactly this.
                tools: self.child_tools.schemas(),
                temperature: provider.as_ref().and_then(|provider| provider.temperature),
                max_tokens: provider.as_ref().and_then(|provider| provider.max_tokens),
                reasoning_effort: provider
                    .as_ref()
                    .and_then(|provider| provider.reasoning_effort.clone()),
            },
            machine: TurnMachine::new(limits_for_turn(&snapshot, provider.as_ref())),
            chunk_buffer: chunk_buffer_for_turn(&snapshot),
            concurrency: concurrency_table(&self.child_tools),
            steering: steer,
            // Child runs keep text-only steers: children have no attachment
            // association surface and their seed carries no image resolver.
            steer_attachment_resolver: None,
        });
        reactor.push_user(ctx.prompt.clone());
        let outcome = reactor.run().await;

        // Real counters only: the reactor folds every provider sample into its
        // machine, so the report cannot invent tokens.
        let progress = reactor.machine().progress();
        let (usage, tool_invocations) = self
            .observed_counts(thread_id, progress.tokens_used, progress.tool_calls)
            .await;

        // The parent reads this run's last assistant message; a run that never
        // produced one reports no summary instead of reusing an earlier turn.
        let summary = match &outcome {
            Ok(TurnOutcome::Completed) | Ok(TurnOutcome::Aborted { .. }) => {
                self.final_answer(thread_id, turn_id).await
            }
            _ => String::new(),
        };
        Ok(child_report(outcome, summary, usage, tool_invocations))
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

    /// The child's last assistant message from this run, which is what the
    /// parent reads.
    async fn final_answer(&self, thread_id: ThreadId, turn_id: TurnId) -> String {
        let items = match self.store.read_items(thread_id).await {
            Ok(items) => items,
            Err(_) => return String::new(),
        };
        items
            .iter()
            .rev()
            .find(|item| item.kind == ItemKind::AssistantMessage && item.turn_id == turn_id)
            .and_then(|item| item.text.clone())
            .unwrap_or_default()
    }
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
            default_background: settings.subagents.default_background,
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
    use super::*;
    use ark_common::ChildId;
    use ark_ports::SubagentPolicySource;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FirstAnswerThenPendingModel(AtomicUsize);

    #[async_trait::async_trait]
    impl ModelClient for FirstAnswerThenPendingModel {
        fn provider_id(&self) -> &str {
            "test"
        }

        fn model(&self) -> &str {
            "test"
        }

        async fn stream(
            &self,
            _request: ark_ports::ModelRequest,
            _sink: tokio::sync::mpsc::Sender<ark_ports::ModelChunk>,
        ) -> Result<ark_ports::ModelResponse> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(ark_ports::ModelResponse {
                    text: "previous answer".to_string(),
                    ..ark_ports::ModelResponse::default()
                })
            } else {
                std::future::pending().await
            }
        }
    }

    fn executor_with_model(
        store: Arc<ark_store::FileSessionStore>,
        dir: &std::path::Path,
        model: Arc<dyn ModelClient>,
    ) -> ChildTurnExecutorImpl {
        let usage = Arc::new(UsageRecorder::new());
        let providers = ark_provider::ProviderStore::new(dir.to_path_buf());
        let skills = Arc::new(ark_skills::SkillRuntime::new(
            Arc::new(ark_skills::SkillsConfig::default),
            dir.to_path_buf(),
        ));
        let roots = Arc::new(WorkspaceRoots::new(dir.to_path_buf(), dir).unwrap());
        ChildTurnExecutorImpl::new(
            Arc::new(ark_config::ConfigStore::new(RuntimeConfig::default())),
            Arc::new(providers),
            usage,
            model,
            Arc::new(ToolRegistry::new()),
            Arc::clone(&store) as Arc<dyn EventSink>,
            store,
            Arc::new(ark_tool::BuiltinSandboxPolicy::new(
                ark_tool::Autonomy::WorkspaceWrite,
            )),
            roots,
            skills,
            Arc::new(ark_ports::NoHooks),
        )
    }

    #[tokio::test]
    async fn resumed_child_abort_does_not_reuse_the_previous_turn_summary() {
        use ark_ports::SessionStore;

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            ark_store::FileSessionStore::open(ark_store::StoreConfig::new(dir.path())).unwrap(),
        );
        let parent = store.create_thread("parent").await.unwrap().thread_id;
        let child = ChildId::new();
        store
            .create_child_run(parent, child, "child")
            .await
            .unwrap();
        let executor = executor_with_model(
            Arc::clone(&store),
            dir.path(),
            Arc::new(FirstAnswerThenPendingModel(AtomicUsize::new(0))),
        );
        let ctx = ChildTurnContext {
            child_id: child,
            parent_thread_id: parent,
            parent_turn_id: TurnId::new(),
            prompt: "initial task".to_string(),
            workspace: None,
            model: None,
            provider_id: None,
            autonomy: None,
            cancel: Arc::new(ark_ports::CancelToken::new()),
            steer: Arc::new(ark_ports::SteerQueue::new()),
        };

        let first = executor.run_child_turn(ctx.clone()).await.unwrap();
        assert_eq!(first.status, ChildTurnStatus::Completed);
        assert_eq!(first.summary, "previous answer");

        let cancel = Arc::new(ark_ports::CancelToken::new());
        cancel.cancel();
        let resumed = ChildTurnContext {
            parent_turn_id: TurnId::new(),
            prompt: "resume after abort".to_string(),
            cancel,
            ..ctx
        };
        let report = executor.run_child_turn(resumed).await.unwrap();

        assert!(matches!(report.status, ChildTurnStatus::Aborted { .. }));
        assert!(
            report.summary.is_empty(),
            "an aborted resume must not report the previous turn's answer: {}",
            report.summary
        );
    }

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

    fn policy_for(enabled: bool, max_concurrency: u32) -> ark_ports::SubagentPolicy {
        let config = RuntimeConfig {
            subagents: ark_contract::SubagentSettings {
                enabled,
                max_concurrency,
                auto_continue: false,
                default_background: true,
            },
            ..RuntimeConfig::default()
        };
        let store = Arc::new(ark_config::ConfigStore::new(config));
        ConfigSubagentPolicy::new(store).policy()
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

    /// The delegate-time guard: an override model the provider's inventory
    /// does not list fails at `provider_for` with a diagnostic naming the
    /// served models, before any model request is attempted (the incident
    /// that motivated this guard never reached the provider (it 404'd at
    /// the gateway with no hint about the mismatch).
    #[test]
    fn provider_for_rejects_a_model_outside_the_provider_inventory() {
        use ark_contract::ProviderModelInventoryItem;

        let dir =
            std::env::temp_dir().join(format!("ark-child-guard-{}", ark_common::ThreadId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = ark_provider::ProviderStore::new(dir.clone());
        let mut entry = ark_contract::ProviderEntry::new("brgroup", "glm-5-3-flash");
        entry.base_url = "https://gateway.invalid".into();
        entry.api_key = Some("sk-test-1234567890".into());
        entry.endpoint_format = "openai_chat".into();
        entry.models = Some(vec![ProviderModelInventoryItem {
            id: "glm-5-2".into(),
            context_window_tokens: Some(200_000),
            supports_vision: None,
        }]);
        store.upsert(entry, true).unwrap();

        let snapshot_store = Arc::new(ark_config::ConfigStore::new(RuntimeConfig::default()));
        let usage = Arc::new(UsageRecorder::new());
        // The parent fallback never runs in this scenario, so a minimal
        // config pointing at an unroutable address is fine.
        let parent_config = ark_llm::ProviderConfig::new("test-parent", "test-model");
        let parent_model = model::select_from_config(parent_config, Arc::clone(&usage)).0;
        let skills = Arc::new(ark_skills::SkillRuntime::new(
            Arc::new(ark_skills::SkillsConfig::default),
            dir.clone(),
        ));
        let roots = Arc::new(WorkspaceRoots::new(dir.clone(), &dir).unwrap());
        let session_store =
            Arc::new(ark_store::FileSessionStore::open(ark_store::StoreConfig::new(&dir)).unwrap());
        let executor = ChildTurnExecutorImpl::new(
            Arc::clone(&snapshot_store),
            Arc::new(store),
            usage,
            parent_model,
            Arc::new(ToolRegistry::new()),
            // The sink only needs to accept constructor-time calls; the store
            // is the production EventSink.
            Arc::clone(&session_store) as Arc<dyn EventSink>,
            session_store,
            Arc::new(ark_tool::BuiltinSandboxPolicy::new(
                ark_tool::Autonomy::WorkspaceWrite,
            )),
            roots,
            skills,
            Arc::new(ark_ports::NoHooks),
        );

        let error = executor
            .provider_for(Some("brgroup"), Some("global:deepseek-v4.1-flash"))
            .unwrap_err();
        assert!(
            error.message.contains("does not serve model"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("glm-5-3-flash") && error.message.contains("glm-5-2"),
            "the served models must be listed: {}",
            error.message
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// R5 regression: with no provider table at all, the child falls through
    /// to the parent's model (`Ok(None)`) instead of failing the run. The
    /// executor is not needed beyond construction; the assertion is the
    /// provider resolution itself.
    #[test]
    fn provider_for_falls_through_to_the_parent_model_without_a_table() {
        let dir = std::env::temp_dir().join(format!(
            "ark-child-guard-fallback-{}",
            ark_common::ThreadId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let providers = ark_provider::ProviderStore::new(dir.clone());

        let usage = Arc::new(UsageRecorder::new());
        let parent_config = ark_llm::ProviderConfig::new("test-parent", "test-model");
        let parent_model = model::select_from_config(parent_config, Arc::clone(&usage)).0;
        let snapshot_store = Arc::new(ark_config::ConfigStore::new(RuntimeConfig::default()));
        let skills = Arc::new(ark_skills::SkillRuntime::new(
            Arc::new(ark_skills::SkillsConfig::default),
            dir.clone(),
        ));
        let roots = Arc::new(WorkspaceRoots::new(dir.clone(), &dir).unwrap());
        let session_store =
            Arc::new(ark_store::FileSessionStore::open(ark_store::StoreConfig::new(&dir)).unwrap());
        let executor = ChildTurnExecutorImpl::new(
            snapshot_store,
            Arc::new(providers),
            usage,
            parent_model,
            Arc::new(ToolRegistry::new()),
            Arc::clone(&session_store) as Arc<dyn EventSink>,
            session_store,
            Arc::new(ark_tool::BuiltinSandboxPolicy::new(
                ark_tool::Autonomy::WorkspaceWrite,
            )),
            roots,
            skills,
            Arc::new(ark_ports::NoHooks),
        );

        // No table entry and no explicit provider: the child inherits the
        // parent's client instead of failing with "no usable default
        // provider".
        assert!(executor.provider_for(None, None).unwrap().is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
