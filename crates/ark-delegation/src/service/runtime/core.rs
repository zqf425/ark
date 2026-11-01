//! `RuntimeCore`: the mutable state machine behind one delegation runtime.
//!
//! Prepare/resume/execute/record/publish live here so the public facade in the
//! parent module stays readable. Field access is inherited from the parent
//! module, which owns the struct; only the entry points the facade calls are
//! `pub(super)`.

use super::*;

impl RuntimeCore {
    pub(super) fn lock_handles(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<ChildId, Arc<RunningChild>>> {
        self.handles.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn lock_known(&self) -> std::sync::MutexGuard<'_, HashMap<ChildId, ChildRunRecord>> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn lock_prepared(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<ChildId, PreparedChild>> {
        self.prepared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_deleted(&self) -> std::sync::MutexGuard<'_, HashSet<ChildId>> {
        self.deleted.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_deleted(&self, child: ChildId) -> bool {
        self.lock_deleted().contains(&child)
    }

    /// Attach runtime-owned control handles before a worker task can be polled.
    ///
    /// This closes the accepted-but-not-abortable window for background and
    /// resumed children: once the caller receives the child id, the handle is
    /// already present in the registry.
    pub(super) fn install_handle(&self, prepared: &mut PreparedChild) {
        let cancel = Arc::new(TokenCancel::default());
        let steer = Arc::new(QueueSteer::default());
        prepared.ctx.cancel = Arc::clone(&cancel) as Arc<dyn CancelHandle>;
        prepared.ctx.steer = Arc::clone(&steer) as Arc<dyn SteerHandle>;
        self.lock_handles().insert(
            prepared.record.child_id,
            Arc::new(RunningChild {
                child_id: prepared.record.child_id,
                parent_thread_id: prepared.record.parent_thread_id,
                cancel,
                steer,
            }),
        );
    }

    /// Create the child session and its `queued` record.
    ///
    /// This is the only place a child id is minted, so depth policy, directory
    /// creation and the first log record can never drift apart.
    pub(super) async fn prepare(&self, request: SubagentRequest) -> Result<PreparedChild> {
        let policy = self.policy.policy();
        if !policy.enabled {
            return Err(ArkError::new(
                ArkErrorCode::Conflict,
                "sub-agents are disabled; enable them in Settings to delegate",
            ));
        }
        let prompt = request.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(ArkError::validation(
                "delegate_task requires a non-empty prompt",
            ));
        }

        // Resume path: continue an interrupted run on its original session.
        if let Some(child_id) = request.resume_child_id {
            return self.prepare_resume(child_id, request, prompt, policy).await;
        }

        let child_id = ChildId::new();
        let label = if request.label.trim().is_empty() {
            default_label(&prompt)
        } else {
            request.label.trim().to_string()
        };
        // Enforces the depth policy: a child of a child is refused here.
        self.store
            .create_child_run(request.parent_thread_id, child_id, &label)
            .await?;

        let mut record = ChildRunRecord::queued(
            child_id,
            request.parent_thread_id,
            request.parent_turn_id,
            label,
            Utc::now(),
        );
        record.prompt = Some(prompt.clone());
        record.model = request.model.clone();
        record.provider_id = request.provider_id.clone();
        record.autonomy = request.autonomy.clone();
        self.store.append_child_run(record.clone()).await?;
        self.lock_known().insert(child_id, record.clone());
        self.publish_activity(&record).await;

        Ok(PreparedChild {
            record,
            ctx: child_context(&request, child_id, prompt),
            policy,
        })
    }

    async fn prepare_resume(
        &self,
        child_id: ChildId,
        request: SubagentRequest,
        prompt: String,
        policy: SubagentPolicy,
    ) -> Result<PreparedChild> {
        // One lock acquisition: taking `known` twice around a non-reentrant mutex
        // would self-deadlock on an unknown id.
        let existing = self.lock_known().get(&child_id).cloned();
        let Some(existing) = existing else {
            return Err(ArkError::not_found(format!(
                "child run not found: {child_id}"
            )));
        };
        if existing.parent_thread_id != request.parent_thread_id {
            // Ownership is decided by the authoritative log, never by the caller.
            return Err(ArkError::not_found(format!(
                "child run not found: {child_id}"
            )));
        }
        if existing.is_active() {
            return Err(ArkError::conflict(format!(
                "child run {child_id} is still {} and cannot be resumed",
                existing.status
            )));
        }
        if existing.status == ark_ports::child_status::COMPLETED {
            return Err(ArkError::conflict(format!(
                "child run {child_id} already completed; nothing to resume"
            )));
        }
        let resume_count = existing.resume_count.unwrap_or(0) + 1;
        let mut record = existing.clone();
        record.status = ark_ports::child_status::QUEUED.to_string();
        record.ended_at = None;
        record.error = None;
        record.summary = None;
        record.resume_count = Some(resume_count);
        record.prompt = Some(prompt.clone());
        self.store.append_child_run(record.clone()).await?;
        self.lock_known().insert(child_id, record.clone());
        self.publish_activity(&record).await;

        let ctx = ChildTurnContext {
            child_id,
            parent_thread_id: request.parent_thread_id,
            parent_turn_id: request.parent_turn_id,
            prompt,
            workspace: request.workspace.map(std::path::PathBuf::from),
            model: request.model.or(existing.model),
            provider_id: request.provider_id.or(existing.provider_id),
            autonomy: request.autonomy.or(existing.autonomy),
            cancel: Arc::new(NoCancel),
            steer: Arc::new(NoSteer),
        };

        Ok(PreparedChild {
            record,
            ctx,
            policy,
        })
    }

    /// Run a prepared child to a terminal state, enforcing the budget.
    ///
    /// Two invariants hold on **every** exit path, including a panicking
    /// executor: the control handle and the `active` counter are released, and
    /// the run reaches a terminal record.
    pub(super) async fn execute(&self, prepared: PreparedChild) -> Result<SubagentOutcome> {
        let PreparedChild {
            record,
            ctx,
            policy,
        } = prepared;
        let child = record.child_id;
        let limit = policy.max_concurrency.max(1);
        let max_queued = policy.max_queued;

        // `start_supervised` installed this handle synchronously before spawning
        // the worker. Public `execute` uses the same path.
        let cancel = Arc::clone(&ctx.cancel);

        // Claim a slot, but let an abort win while we wait. Dropping a pending
        // acquire is safe: the gate skips waiters whose receiver went away.
        let cancel_for_wait = Arc::clone(&cancel);
        let acquire = self.slot_gate.acquire(limit as usize, max_queued as usize);
        tokio::pin!(acquire);
        let guard = tokio::select! {
            acquired = &mut acquire => match acquired {
                Ok(guard) => guard,
                Err(err) => {
                    self.lock_handles().remove(&child);
                    // Never leave the `queued` record behind: a saturated budget
                    // is a caller-visible conflict, not a run that still waits.
                    let _ = self
                        .finish(record, ChildTurnReport::failed(err.message.clone()))
                        .await;
                    self.settled.notify_waiters();
                    return Err(err);
                }
            },
            _ = wait_cancelled(cancel_for_wait) => {
                self.lock_handles().remove(&child);
                let aborted = self
                    .finish(
                        record,
                        ChildTurnReport::aborted("aborted while waiting for a slot"),
                    )
                    .await?;
                self.settled.notify_waiters();
                return Ok(outcome_of(&aborted));
            }
        };
        self.active.fetch_add(1, Ordering::Relaxed);
        // Releases the handle and the counter on every path, including unwind.
        let slot = RunSlot {
            handles: Arc::clone(&self.handles),
            active: Arc::clone(&self.active),
            child,
        };

        // A cancel that landed between acquiring the slot and here must not start
        // a turn nobody is waiting for any more.
        if cancel.is_cancelled() {
            drop(slot);
            drop(guard);
            let aborted = self
                .finish(
                    record,
                    ChildTurnReport::aborted("aborted before the turn started"),
                )
                .await?;
            self.settled.notify_waiters();
            return Ok(outcome_of(&aborted));
        }

        let started = match self
            .relabel(&record, ark_ports::child_status::RUNNING)
            .await
        {
            Ok(started) => started,
            Err(err) => {
                // The run never really started, so it must not stay `queued`.
                let _ = self
                    .finish(record, ChildTurnReport::failed(err.message.clone()))
                    .await;
                drop(slot);
                drop(guard);
                self.settled.notify_waiters();
                return Err(err);
            }
        };
        self.publish_activity(&started).await;

        let report = match self.executor.run_child_turn(ctx).await {
            Ok(report) => report,
            Err(err) => ChildTurnReport::failed(err.message),
        };

        let finished = self.finish(started, report).await?;
        drop(slot);
        drop(guard);
        self.settled.notify_waiters();
        Ok(outcome_of(&finished))
    }

    /// Append a non-terminal status change.
    ///
    /// Refuses (and reports) a run the user deleted while it was starting, so a
    /// late `relabel` cannot resurrect a destroyed child.
    async fn relabel(&self, record: &ChildRunRecord, status: &str) -> Result<ChildRunRecord> {
        if self.is_deleted(record.child_id) {
            return Err(ArkError::not_found(format!(
                "child run {} was deleted",
                record.child_id
            )));
        }
        let mut next = record.clone();
        next.status = status.to_string();
        self.store.append_child_run(next.clone()).await?;
        self.lock_known().insert(next.child_id, next.clone());
        Ok(next)
    }

    /// Append the terminal record and publish the activity event.
    ///
    /// Convergence beats propagation. If the append fails, the in-memory
    /// registry is still moved to the terminal state and the failure is logged:
    /// leaving `known` at `running` would make the API report a run that nothing
    /// is executing, while `abort` finds no handle and `resume`/`destroy` refuse
    /// because the record still looks active — until a restart (P1-3). The
    /// authoritative log keeps the older row, and `hydrate` converges it on the
    /// next start.
    pub(super) async fn finish(
        &self,
        record: ChildRunRecord,
        report: ChildTurnReport,
    ) -> Result<ChildRunRecord> {
        let finished = finished_record(record, &report, Utc::now());
        if self.is_deleted(finished.child_id) {
            // The user destroyed this run while it was finishing; honour that and
            // keep the in-memory view consistent with it.
            self.lock_known().remove(&finished.child_id);
            return Ok(finished);
        }
        if let Err(err) = self.store.append_child_run(finished.clone()).await {
            tracing::warn!(
                child = %finished.child_id,
                error = %err.message,
                "could not persist a terminal child-run record; converging in memory only"
            );
        }
        self.lock_known()
            .insert(finished.child_id, finished.clone());
        self.publish_activity(&finished).await;
        self.publish_completion(&finished).await;
        Ok(finished)
    }

    /// Emit `child_activity` on the **parent** stream.
    ///
    /// The parent UI subscribes to its own thread; making it watch every child
    /// stream would multiply connections by the number of children.
    async fn publish_activity(&self, record: &ChildRunRecord) {
        let event = RuntimeEvent::ChildActivity {
            seq: 0,
            thread_id: record.parent_thread_id,
            timestamp: Utc::now(),
            child_id: record.child_id,
            label: record.label.clone(),
            status: record.status.clone(),
        };
        // Activity events are advisory: failing to publish one must never fail
        // the child run itself.
        if let Err(err) = self.sink.publish(record.parent_thread_id, event).await {
            tracing::warn!(
                child = %record.child_id,
                error = %err.message,
                "child_activity event was not published"
            );
        }
    }

    /// Publish the terminal result separately from lifecycle progress so the
    /// parent can receive an actionable notification without reading the child
    /// transcript. The id is stable across a retry of the same run state.
    async fn publish_completion(&self, record: &ChildRunRecord) {
        if record.is_active() {
            return;
        }
        let notification_id = completion_notification_id(record);
        match self
            .completion_already_published(record, &notification_id)
            .await
        {
            Ok(true) => return,
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(
                    child = %record.child_id,
                    error = %err.message,
                    "could not inspect child completion delivery state"
                );
                return;
            }
        }
        let event = completion_event(record, notification_id);
        match self.sink.publish(record.parent_thread_id, event).await {
            Ok(_) => {
                let _ = self.completions.send(record.parent_thread_id);
            }
            Err(err) => {
                tracing::warn!(
                    child = %record.child_id,
                    error = %err.message,
                    "child completion notification was not published"
                );
            }
        }
    }

    async fn completion_already_published(
        &self,
        record: &ChildRunRecord,
        notification_id: &str,
    ) -> Result<bool> {
        let events = self.store.replay_events(record.parent_thread_id, 0).await?;
        Ok(events.into_iter().any(|event| {
            matches!(
                event,
                RuntimeEvent::ChildCompleted {
                    notification_id: existing,
                    ..
                } if existing == notification_id
            )
        }))
    }

    pub(super) async fn hydrate(&self) -> Result<u32> {
        let mut latest: HashMap<ChildId, ChildRunRecord> = HashMap::new();
        for meta in self.store.list_threads().await? {
            // A damaged parent is skipped rather than aborting the whole
            // hydration: the remaining children are still worth serving.
            let Ok(records) = self.store.list_child_runs(meta.thread_id).await else {
                continue;
            };
            for record in records {
                match latest.get(&record.child_id) {
                    // The log is append-only, so the newest row wins.
                    Some(current) if current.created_at > record.created_at => {}
                    _ => {
                        latest.insert(record.child_id, record);
                    }
                }
            }
        }

        let mut loaded = 0u32;
        for (child, record) in latest {
            loaded += 1;
            self.lock_known().insert(child, record.clone());
            if !record.is_active() {
                self.publish_completion(&record).await;
                continue;
            }
            let terminal = finished_record(
                record,
                &ChildTurnReport::failed("interrupted by a runtime restart"),
                Utc::now(),
            );
            // Converge memory before attempting persistence. A write failure must
            // not leave the live registry reporting an orphan as queued/running;
            // the durable active row remains available for another retry on the
            // next start.
            self.lock_known().insert(child, terminal.clone());
            if let Err(err) = self.store.append_child_run(terminal.clone()).await {
                tracing::warn!(
                    child = %child,
                    error = %err.message,
                    "could not persist convergence of an orphaned child run"
                );
            } else {
                self.publish_activity(&terminal).await;
                self.publish_completion(&terminal).await;
            }
        }
        Ok(loaded)
    }

    pub(super) async fn destroy_child(&self, parent: ThreadId, child: ChildId) -> Result<bool> {
        let record = self.lock_known().get(&child).cloned();
        let Some(record) = record else {
            return Ok(false);
        };
        if record.parent_thread_id != parent {
            // Ownership is decided by the authoritative record, never the caller.
            return Ok(false);
        }
        if record.is_active() || self.lock_handles().contains_key(&child) {
            return Err(ArkError::conflict(format!(
                "child run {child} is still {} and cannot be destroyed; abort it first",
                record.status
            )));
        }
        // Mark first so a late `finish()` cannot recreate the record. Keep the
        // known record until storage succeeds so an I/O failure does not make a
        // still-present child disappear from the API.
        self.lock_deleted().insert(child);
        self.lock_prepared().remove(&child);
        let removed = match self.store.delete_child_run(parent, child).await {
            Ok(removed) => removed,
            Err(err) => {
                self.lock_deleted().remove(&child);
                return Err(err);
            }
        };
        if !removed {
            self.lock_deleted().remove(&child);
            return Ok(false);
        }
        self.lock_known().remove(&child);
        Ok(true)
    }
}
