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
    build_reactor, chunk_buffer_for_turn, limits_for_turn, push_project_instructions,
    resolve_workspace, seed_prefix, skill_tail, HookToolRunner, HookedToolRunner, ReactorSpec,
    CHILD_ROLE_LINE,
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
        let model = model
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        match self.providers.resolve_for_model(provider_id, model) {
            Some(provider) => Ok(Some(provider)),
            // Distinguish "no provider configured" (fall through to the parent
            // fallback) from "the named provider is unusable" (an error).
            None if provider_id.is_none() => Ok(None),
            None => Err(ark_common::ArkError::validation(format!(
                "child provider is unavailable: {}",
                provider_id.unwrap_or_default()
            ))),
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
        // limits must come from the same config state.
        let snapshot = self.config.snapshot();
        let settings = self.inherited_settings(&ctx, &snapshot).await?;
        let workspace = settings.workspace;
        let sandbox: Arc<dyn SandboxPolicy> =
            Arc::new(ark_tool::BuiltinSandboxPolicy::new(settings.autonomy));
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
            steering: steer,
        });
        reactor.push_user(ctx.prompt.clone());
        let outcome = reactor.run().await;

        // Real counters only: the reactor folds every provider sample into its
        // machine, so the report cannot invent tokens.
        let progress = reactor.machine().progress();
        let (usage, tool_invocations) = self
            .observed_counts(thread_id, progress.tokens_used, progress.tool_calls)
            .await;

        // The parent reads the child's last assistant message; a run that never
        // produced one reports no summary instead of an invented placeholder.
        let summary = match &outcome {
            Ok(TurnOutcome::Completed) | Ok(TurnOutcome::Aborted { .. }) => {
                self.final_answer(thread_id).await
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
