//! One parent turn: the reactor wired to the real store, tools and gates.

use super::history::{conversation_history, workspace_line};
use super::*;

/// One turn, driven by the real reactor (`ark-agent`), with the real store and
/// the real tool registry behind the ports.
pub(super) struct AgentTurnRunner {
    /// Kept so a turn can resolve its model from the **current** provider table
    /// and config snapshot. Holding one client for the process lifetime made
    /// `Applied` a lie: editing a provider or `tuning` never reached the next
    /// parent turn (design §7.15, P1-7).
    pub(super) providers: Arc<ark_provider::ProviderStore>,
    pub(super) config: Arc<ConfigStore>,
    pub(super) usage: Arc<UsageRecorder>,
    pub(super) tools: Arc<RegistryToolRunner>,
    pub(super) store: Arc<dyn SessionStore>,
    pub(super) sink: Arc<dyn EventSink>,
    /// Resolved per turn so a workspace chosen in Settings applies immediately.
    pub(super) roots: Arc<WorkspaceRoots>,
    pub(super) tool_registry: Arc<ToolRegistry>,
    /// Skill runtime, consulted once per turn for activated instructions.
    pub(super) skills: Arc<ark_skills::SkillRuntime>,
    /// Long-term memory document. Re-read once per turn so a memory the user
    /// just saved is visible to the very next turn without a restart.
    pub(super) memory: Arc<ark_store::MemoryFileStore>,
    /// Human gate broker shared with the HTTP command surface.
    pub(super) gates: Arc<dyn GateBroker>,
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
    pub(super) fn limits_for_turn(
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
pub(super) async fn completion_inbox(
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
pub(super) async fn abort_interrupted_turns(store: &dyn SessionStore) -> Result<usize> {
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
pub(super) async fn finish_parent_turn(
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
        // The attachment ids ride the same record: the route already proved they
        // are associated with this thread, and recording them is the audit trail
        // that makes "which turn referenced this file" replayable.
        let events = if is_auto {
            TurnEvents::auto_continuation(job.thread_id, job.turn_id)
        } else {
            TurnEvents::new(job.thread_id, job.turn_id)
        }
        .with_attachments(job.attachment_ids);
        let writer = Arc::new(EventSinkWriter::new(Arc::clone(&self.sink), events.clone()));
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
