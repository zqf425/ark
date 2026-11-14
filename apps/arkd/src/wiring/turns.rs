//! One parent turn: the reactor wired to the real store, tools and gates.

use super::history::conversation_history;
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
    /// Attachment bytes. Only the composition root touches them: phase two
    /// resolves authorized ids into model-seed parts here, never in the agent.
    pub(super) attachments: Arc<dyn ark_ports::AttachmentStore>,
    /// Human gate broker shared with the HTTP command surface.
    pub(super) gates: Arc<dyn GateBroker>,
    /// Hook bus (requirements/plugin-hook-bus.md). Shared with the child
    /// executor and the AppState, so one broker instance owns every decision.
    pub(super) hooks: Arc<dyn ark_ports::HookBroker>,
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
        thread_id: ThreadId,
        turn_id: TurnId,
    ) -> Arc<dyn ModelClient> {
        let inner = match provider {
            Some(endpoint) => model::select_from_config(endpoint, Arc::clone(&self.usage)).0,
            None => model::select(snapshot, Arc::clone(&self.usage)).0,
        };
        model::hooked(Arc::clone(&self.hooks), thread_id, turn_id, inner)
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
                        reason: ark_common::CancelReason::RuntimeRestart
                            .as_durable_str()
                            .to_string(),
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
        // The durable plan is re-read every turn so restating the goal/todos in
        // the recent tail cannot drift from what the workbench shows.
        let plan = ark_server::service::workbench::plan(
            &self.store.replay_events(job.thread_id, 0).await?,
        );
        let thread = self.store.get_thread(job.thread_id).await?;
        let (source_text, user_text) = turn_source_texts(&job.source);
        // Capture once before building either the prompt or tool adapter. A
        // per-session override wins, otherwise the process workspace lease is
        // used; neither can change during this turn.
        let workspace = super::turn_common::resolve_workspace(
            thread
                .runtime
                .workspace
                .as_deref()
                .map(std::path::Path::new),
            None,
            self.roots.lease().canonical_root,
            "thread",
        )?;
        let snapshot = self.config.snapshot();
        let (attachments, events, writer) = self
            .parent_delivery(
                &job,
                thread.runtime.provider_id.as_deref(),
                thread.runtime.model.as_deref(),
            )
            .await?;
        let policy: Arc<dyn SandboxPolicy> = Arc::new(BuiltinSandboxPolicy::new(
            autonomy_from_override_or_default(thread.runtime.autonomy.as_deref(), &snapshot)?,
        ));
        // Plugin decide first, then the shell `pre_tool_use` hook, then
        // dispatch (requirements/plugin-hook-bus.md, pinned order).
        let tools: Arc<dyn AgentToolRunner> = Arc::new(super::turn_common::HookToolRunner::new(
            Arc::clone(&self.hooks),
            job.thread_id,
            job.turn_id,
            super::turn_common::HookedToolRunner::new(
                Arc::clone(&self.tools),
                Arc::clone(&self.config),
                workspace.clone(),
                job.thread_id,
                job.turn_id,
            ),
        ));
        let seed_messages = self.parent_seed(&workspace, history, &source_text, &inbox, &plan);
        // One snapshot per turn: every value below is derived from the same
        // config state, so a turn cannot mix a new model with old limits.
        let provider = resolve_thread_provider(
            &self.providers,
            thread.runtime.provider_id.as_deref(),
            thread.runtime.model,
            thread.runtime.reasoning_effort,
        )?;
        let model = self.model_for_turn(&snapshot, provider.clone(), job.thread_id, job.turn_id);
        let mut reactor = super::turn_common::build_reactor(super::turn_common::ReactorSpec {
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
                reasoning_effort: provider
                    .as_ref()
                    .and_then(|provider| provider.reasoning_effort.clone()),
            },
            machine: TurnMachine::new(super::turn_common::limits_for_turn(
                &snapshot,
                provider.as_ref(),
            )),
            chunk_buffer: super::turn_common::chunk_buffer_for_turn(&snapshot),
            steering: job.steering,
        });
        if let Some(text) = user_text {
            reactor.push_user_message(user_message_with_attachments(text, &attachments));
        }
        let outcome = reactor.run().await?;
        let end = finish_parent_turn(
            outcome,
            self.store.as_ref(),
            self.sink.as_ref(),
            job.thread_id,
            job.turn_id,
            &inbox,
        )
        .await;
        Ok(end)
    }
}

impl AgentTurnRunner {
    /// Resolve the turn's attachments and the durable `turn_started` event.
    ///
    /// Attachment phase two: the ids the route already authorized become seed
    /// parts *before* `turn_started` is written, so the delivery outcome rides
    /// the same durable record. Only an explicit catalog-reported
    /// `supports_vision` lets images through. Auto-continuations are marked on
    /// the same event so per-notification delivery accounting survives
    /// restarts; the attachment ids are the audit trail that makes "which turn
    /// referenced this file" replayable.
    async fn parent_delivery(
        &self,
        job: &TurnJob,
        provider_id: Option<&str>,
        model: Option<&str>,
    ) -> Result<(
        crate::attachment_seed::AttachmentSeed,
        TurnEvents,
        Arc<EventSinkWriter>,
    )> {
        let is_auto = matches!(&job.source, TurnSource::AutoContinuation);
        // Vision follows the effective model: a thread override reads the
        // inventory entry recorded for that model when there is one.
        let vision = self.providers.resolve_vision(provider_id, model);
        let attachments = crate::attachment_seed::resolve_attachments(
            self.attachments.as_ref(),
            &job.attachment_ids,
            vision == Some(true),
            crate::attachment_seed::IMAGE_BUDGET_BYTES,
        );
        let events = if is_auto {
            TurnEvents::auto_continuation(job.thread_id, job.turn_id)
        } else {
            TurnEvents::new(job.thread_id, job.turn_id)
        }
        .with_attachments(job.attachment_ids.clone())
        .with_attachment_outcomes(attachments.outcomes.clone());
        let writer = Arc::new(EventSinkWriter::new(Arc::clone(&self.sink), events.clone()));
        Ok((attachments, events, writer))
    }

    /// Assemble the parent turn's seed: static prefix, replayed history, then
    /// the dynamic tail (skills, project instructions, memory, completion
    /// inbox, thread plan).
    ///
    /// Only the static prefix may enter the prompt cache; everything after it
    /// changes between turns, so it must stay out of the prefix. The plan is
    /// last on purpose: it must sit closest to the new user message.
    fn parent_seed(
        &self,
        workspace: &std::path::Path,
        history: Vec<ModelMessage>,
        source_text: &str,
        inbox: &CompletionInbox,
        plan: &ark_contract::PlanView,
    ) -> Vec<ModelMessage> {
        let resolution = self.skills.resolve_for_turn(source_text);
        let mut seed_messages = super::turn_common::seed_prefix(workspace);
        seed_messages.extend(history);
        seed_messages.extend(super::turn_common::skill_tail(&resolution, true));
        super::turn_common::push_project_instructions(workspace, &mut seed_messages);
        if let Some(message) = crate::memory_seed::seed_for_turn(self.memory.as_ref()) {
            seed_messages.push(message);
        }
        if let Some(message) = inbox.system_message() {
            seed_messages.push(message);
        }
        if let Some(message) = super::history::plan_seed_message(plan) {
            seed_messages.push(message);
        }
        seed_messages
    }
}

/// Split the turn source into the skill-resolution text and the user message
/// text (an auto-continuation has neither; an IM message behaves like explicit
/// user input — requirements/im-channels.md).
fn turn_source_texts(source: &TurnSource) -> (String, Option<String>) {
    match source {
        TurnSource::User(text) => (text.clone(), Some(text.clone())),
        TurnSource::Im { text, .. } => (text.clone(), Some(text.clone())),
        TurnSource::AutoContinuation => (String::new(), None),
    }
}

/// Resolve the parent turn's provider from the live table, then apply the
/// thread's model override.
///
/// The provider table wins; `model_providers` in `config.json` remains the
/// legacy fallback (read inside [`AgentTurnRunner::model_for_turn`]). The
/// model override also picks up the inventory entry recorded for that model
/// (context window, vision flag): metadata belongs to the model id, so an
/// override compacts against its own limit, never the stored model's.
fn resolve_thread_provider(
    providers: &ark_provider::ProviderStore,
    provider_id: Option<&str>,
    model: Option<String>,
    reasoning_effort: Option<String>,
) -> Result<Option<ark_llm::ProviderConfig>> {
    let mut provider = match provider_id {
        Some(id) => Some(
            providers
                .resolve_for_model(Some(id), model.clone())
                .ok_or_else(|| {
                    ark_common::ArkError::validation(format!(
                        "thread provider is unavailable: {id}"
                    ))
                })?,
        ),
        None => providers.resolve_for_model(None, model),
    };
    if let Some(provider) = &mut provider {
        // An empty override means "unset": drop it instead of sending an empty
        // reasoning hint to the provider mapper.
        provider.reasoning_effort = reasoning_effort.filter(|value| !value.is_empty());
    }
    Ok(provider)
}

/// Build the user message for one turn from the typed text and the resolved
/// attachment seed.
///
/// Previews ride the message content so the durable item and the replayed
/// history keep the same text the model saw; images ride the multimodal parts
/// and are seed-time only (they are not projected into later history).
pub(super) fn user_message_with_attachments(
    text: String,
    attachments: &crate::attachment_seed::AttachmentSeed,
) -> ModelMessage {
    let mut content = text;
    if !attachments.preview.is_empty() {
        content.push('\n');
        content.push_str(&attachments.preview);
    }
    let mut message = ModelMessage::user(content);
    message.images = attachments.images.clone();
    message
}
