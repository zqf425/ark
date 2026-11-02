//! IM channel orchestration (requirements/im-channels.md).
//!
//! The service owns the *loop* and the *lifecycle*, never the platform
//! protocol: connections come from `ark-im` transports, turns come from
//! `crate::service::turns::start_turn_as`, and this module only decides when a
//! channel runs, how inbound messages reach a turn, and how replies get back.
//!
//! Three rules shape the code:
//!
//! * **A message is a real turn.** Inbound text goes through `start_turn_as`
//!   with [`TurnSource::Im`], so the text limit, the one-turn-per-thread
//!   conflict and the workspace gate apply unchanged, and the text lands as a
//!   persisted user message.
//! * **Reconcile, don't patch.** Channel CRUD and config changes only notify;
//!   one `sync` pass diffs desired channels against running handles and
//!   rebuilds what changed (the `ClawRuntime::sync` shape from smart-ark).
//! * **The registry is the only conflict arbiter.** The per-conversation
//!   queues serialize this service's own dispatch; a turn someone else started
//!   on the bound thread surfaces as `start_turn`'s `Conflict` and is retried
//!   on a bounded budget before the chat gets a busy reply.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ark_common::{ArkError, ArkErrorCode};
use ark_contract::{
    ImChannel, ImChannelId, ImChannelRuntime, ImChannelState, ImWeixinLoginStartResponse,
    ImWeixinLoginStatus, ImWeixinLoginWaitResponse,
};
use ark_im::{build_transport, DedupSet, ImLoginFlow, ImTransport, InboundMessage, LoginPoll};
use ark_ports::{CancelToken, ImChannelRecord, ImStore};
use tokio::sync::{mpsc, Notify};

use crate::state::AppState;

use super::im_inbound::ConversationQueues;

#[cfg(test)]
mod tests;

/// Reply an ack after this many seconds of a still-running turn.
pub const IM_ACK_SECS: u64 = 120;
/// Give up waiting (not the turn itself) after this many seconds.
pub const IM_MAX_WAIT_SECS: u64 = 1800;
/// Wait between retries when the bound thread is running someone else's turn.
pub const IM_BUSY_RETRY_SECS: u64 = 30;
/// Bounded retry budget before the chat gets a busy reply.
pub const IM_BUSY_RETRIES: usize = 5;
/// How often the control loop reconciles channels against config + store.
pub const SYNC_SWEEP_INTERVAL: Duration = Duration::from_secs(30);
/// Per-conversation queue depth before a busy reply.
pub const CONVERSATION_QUEUE_CAP: usize = 4;
/// Cursor persistence: at most one rewrite per sweep window or per N messages.
const CURSOR_PERSIST_INTERVAL: Duration = Duration::from_secs(30);
const CURSOR_PERSIST_MAX_MESSAGES: usize = 10;
/// An `error`-state channel is retried at most this often (the sweep is the
/// backoff; nothing spins inside the loop).
const ERROR_RETRY_COOLDOWN: Duration = Duration::from_secs(60);
/// Interactive login sessions expire after ten minutes, at most one at a time.
const LOGIN_TTL: Duration = Duration::from_secs(600);

type TransportFactory = dyn Fn(
        reqwest::Client,
        ark_contract::ImChannelKind,
        &ark_contract::ImCredentials,
        Option<String>,
    ) -> ark_common::Result<Arc<dyn ImTransport>>
    + Send
    + Sync;

/// A running channel: transport + cancellation + live runtime state.
pub struct ChannelHandle {
    pub transport: Arc<dyn ImTransport>,
    pub cancel: CancelToken,
    pub runtime: Arc<Mutex<ImChannelRuntime>>,
    fingerprint: u64,
    last_started: Instant,
    /// Last cursor value persisted through the store, so the sweep can tell
    /// "advanced" from "already on disk".
    cursor_baseline: Option<String>,
    cursor_messages_since: usize,
    cursor_last_persist: Instant,
}

struct ImRuntimeState {
    channels: HashMap<ImChannelId, ChannelHandle>,
    dedup: HashMap<ImChannelId, DedupSet>,
}

/// IM orchestration: one instance per process, built by the composition root.
pub struct ImService {
    store: Arc<dyn ImStore>,
    /// Injected, governed client (design: adapters never build their own).
    client: reqwest::Client,
    factory: Box<TransportFactory>,
    state: Mutex<ImRuntimeState>,
    sync_notify: Notify,
    pub(crate) conversations: ConversationQueues,
    started: AtomicBool,
    /// Optional interactive login flow (WeChat QR); assembled by the
    /// composition root, absent when the platform adapter is not wired.
    login_flow: Mutex<Option<Arc<dyn ImLoginFlow>>>,
    login_sessions: Mutex<HashMap<String, (ark_im::LoginSession, Instant)>>,
}

impl ImService {
    pub fn new(store: Arc<dyn ImStore>, client: reqwest::Client) -> Arc<Self> {
        Self::with_factory(store, client, Box::new(build_transport))
    }

    /// Test seam: swap the transport factory for a fake platform.
    pub fn with_factory(
        store: Arc<dyn ImStore>,
        client: reqwest::Client,
        factory: Box<TransportFactory>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            client,
            factory,
            state: Mutex::new(ImRuntimeState {
                channels: HashMap::new(),
                dedup: HashMap::new(),
            }),
            sync_notify: Notify::new(),
            conversations: ConversationQueues::new(),
            started: AtomicBool::new(false),
            login_flow: Mutex::new(None),
            login_sessions: Mutex::new(HashMap::new()),
        })
    }

    pub fn store(&self) -> &Arc<dyn ImStore> {
        &self.store
    }

    /// The governed client injected by the composition root; the verify route
    /// builds a throwaway transport with it.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Ask the control loop to reconcile now (channel CRUD, config change).
    pub fn request_sync(&self) {
        self.sync_notify.notify_one();
    }

    /// Attach the interactive login flow (WeChat QR login).
    pub fn set_login_flow(&self, flow: Arc<dyn ImLoginFlow>) {
        *self
            .login_flow
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(flow);
    }

    /// `POST /v1/im/weixin/login/start`: fetch (or reuse) a QR session.
    pub async fn weixin_login_start(&self) -> ark_common::Result<ImWeixinLoginStartResponse> {
        let flow = self
            .login_flow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                ArkError::new(
                    ArkErrorCode::RuntimeUnavailable,
                    "weixin login flow is not wired in this build",
                )
            })?;
        // One live session: a fresh request replaces any stale one instead of
        // piling up QR codes nobody scans.
        {
            let mut sessions = self
                .login_sessions
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            sessions.retain(|_, (_, started_at)| started_at.elapsed() < LOGIN_TTL);
        }
        let session = flow.start().await?;
        self.login_sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(session.id.clone(), (session.clone(), Instant::now()));
        Ok(ImWeixinLoginStartResponse {
            session_id: session.id,
            qr_content: session.qr_content,
            message: "用手机微信扫描二维码以连接。".to_string(),
        })
    }

    /// `GET /v1/im/weixin/login/wait`: poll once; a confirmed login creates
    /// the channel server-side (capacity-checked, disabled until edited).
    pub async fn weixin_login_wait(
        self: &Arc<Self>,
        session_id: &str,
    ) -> ark_common::Result<ImWeixinLoginWaitResponse> {
        let flow = self
            .login_flow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                ArkError::new(
                    ArkErrorCode::RuntimeUnavailable,
                    "weixin login flow is not wired in this build",
                )
            })?;
        let session = {
            let mut sessions = self
                .login_sessions
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match sessions.remove(session_id) {
                Some((session, started_at)) if started_at.elapsed() < LOGIN_TTL => Some(session),
                Some(_) => None, // expired: fall through to the Expired branch
                None => None,
            }
        };
        let Some(session) = session else {
            return Ok(ImWeixinLoginWaitResponse {
                status: ImWeixinLoginStatus::Expired,
                message: "二维码已过期，请重新生成。".to_string(),
                channel: None,
            });
        };
        match flow.poll(&session).await? {
            LoginPoll::Pending => Ok(ImWeixinLoginWaitResponse {
                status: ImWeixinLoginStatus::Pending,
                message: "等待扫码。".to_string(),
                channel: None,
            }),
            LoginPoll::Scanned => Ok(ImWeixinLoginWaitResponse {
                status: ImWeixinLoginStatus::Scanned,
                message: "已扫码，请在手机上确认。".to_string(),
                channel: None,
            }),
            LoginPoll::Expired => Ok(ImWeixinLoginWaitResponse {
                status: ImWeixinLoginStatus::Expired,
                message: "二维码已过期，请重新生成。".to_string(),
                channel: None,
            }),
            LoginPoll::Failed(message) => Ok(ImWeixinLoginWaitResponse {
                status: ImWeixinLoginStatus::Failed,
                message,
                channel: None,
            }),
            LoginPoll::Confirmed { credentials, title } => {
                // The login-created channel follows the normal create rules,
                // including the capacity check: exceeding it fails the login
                // instead of creating a 21st channel.
                let existing = self.store().list_channels()?.len();
                ark_im::ensure_channel_capacity(existing)?;
                let now = chrono::Utc::now();
                let channel = ImChannel {
                    id: ImChannelId::new(),
                    kind: ark_contract::ImChannelKind::Weixin,
                    title,
                    // Created disabled on purpose: the user reviews policy
                    // (and the master switch) before anything connects.
                    enabled: false,
                    credentials: Some(credentials),
                    policy: Default::default(),
                    workspace: None,
                    model: None,
                    provider_id: None,
                    autonomy: None,
                    created_at: now,
                    updated_at: now,
                };
                let stored = self.store().insert_channel(ImChannelRecord {
                    channel,
                    cursor: None,
                })?;
                self.request_sync();
                Ok(ImWeixinLoginWaitResponse {
                    status: ImWeixinLoginStatus::Confirmed,
                    message: "已连接微信；渠道默认停用，请在设置页启用。".to_string(),
                    channel: Some(stored.channel.to_view()),
                })
            }
        }
    }

    /// Live runtime state of one channel, or the disabled default.
    pub fn channel_runtime(&self, id: &ImChannelId) -> ImChannelRuntime {
        self.lock()
            .channels
            .get(id)
            .map(|handle| {
                handle
                    .runtime
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
            })
            .unwrap_or_default()
    }

    fn lock(&self) -> MutexGuard<'_, ImRuntimeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Start the control loop. Called once by `serve` after the listener is
    /// bound; a headless `run` never starts it.
    pub fn spawn(self: &Arc<Self>, state: AppState) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let mut shutdown = state.shutdown_receiver();
            loop {
                if state.is_shutting_down() {
                    break;
                }
                service.sync(&state).await;
                tokio::select! {
                    _ = service.sync_notify.notified() => {}
                    _ = tokio::time::sleep(SYNC_SWEEP_INTERVAL) => {}
                    _ = shutdown.recv() => break,
                }
            }
            service.stop_all();
            tracing::info!("im service stopped");
        });
    }

    /// Diff desired channels against running handles.
    ///
    /// The master switch gates *connections*, never editing: with `im.enabled`
    /// off every handle is stopped and the store stays untouched.
    async fn sync(self: &Arc<Self>, state: &AppState) {
        let master_enabled = state.config.snapshot().im.enabled;
        let records = match self.store.list_channels() {
            Ok(records) => records,
            Err(err) => {
                tracing::warn!(error = %err.message, "cannot read im channels during sync");
                return;
            }
        };
        let mut desired: HashMap<ImChannelId, &ImChannel> = HashMap::new();
        for record in &records {
            let channel = &record.channel;
            let startable = master_enabled
                && channel.enabled
                && channel
                    .credentials
                    .as_ref()
                    .is_some_and(ark_contract::ImCredentials::secrets_present);
            if startable {
                desired.insert(channel.id.clone(), channel);
            }
        }

        // Stop handles that are no longer desired (or whose config changed).
        let stale: Vec<ImChannelId> = self
            .lock()
            .channels
            .iter()
            .filter(|(id, handle)| match desired.get(id) {
                Some(channel) => fingerprint(channel) != handle.fingerprint,
                None => true,
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &stale {
            if let Some(handle) = self.lock().channels.remove(id) {
                handle.cancel.cancel();
                tracing::info!(channel = %id, "im channel stopped for reconcile");
            }
        }

        // Start missing/changed channels; retry errored ones on a cooldown.
        for (id, channel) in &desired {
            let existing = self.lock().channels.get(id).map(|handle| {
                (
                    handle.fingerprint,
                    handle.last_started,
                    handle
                        .runtime
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .state,
                )
            });
            let needs_start = match existing {
                None => true,
                Some((_, _, ImChannelState::Error)) => self
                    .lock()
                    .channels
                    .get(id)
                    .is_some_and(|handle| handle.last_started.elapsed() >= ERROR_RETRY_COOLDOWN),
                Some(_) => false,
            };
            if !needs_start {
                continue;
            }
            if let Some(handle) = existing.filter(|(_, _, state)| *state == ImChannelState::Error) {
                let _ = handle;
                if let Some(old) = self.lock().channels.remove(id) {
                    old.cancel.cancel();
                }
            }
            if let Err(err) = self.start_channel(state, channel).await {
                tracing::warn!(channel = %id, error = %err.message, "cannot start im channel");
            }
        }

        self.persist_cursors(&records);
    }

    async fn start_channel(
        self: &Arc<Self>,
        state: &AppState,
        channel: &ImChannel,
    ) -> ark_common::Result<()> {
        let credentials = channel
            .credentials
            .as_ref()
            .ok_or_else(|| ark_common::ArkError::validation("channel has no credentials"))?;
        let record = self.store.get_channel(&channel.id)?;
        let transport = (self.factory)(
            self.client.clone(),
            channel.kind,
            credentials,
            record.cursor.clone(),
        )?;
        let cancel = CancelToken::new();
        let runtime = Arc::new(Mutex::new(ImChannelRuntime {
            state: ImChannelState::Connecting,
            last_error: None,
            last_inbound_at: None,
        }));
        let (inbound_tx, mut inbound_rx) = mpsc::channel::<InboundMessage>(32);
        let handle = ChannelHandle {
            transport: Arc::clone(&transport),
            cancel: cancel.clone(),
            runtime: Arc::clone(&runtime),
            fingerprint: fingerprint(channel),
            last_started: Instant::now(),
            cursor_baseline: record.cursor.clone(),
            cursor_messages_since: 0,
            cursor_last_persist: Instant::now(),
        };
        self.lock().channels.insert(channel.id.clone(), handle);

        let service = self.self_arc();
        let ctx = Arc::new(super::im_inbound::ChannelContext {
            channel: channel.clone(),
            transport: Arc::clone(&transport),
            runtime: Arc::clone(&runtime),
        });
        let loop_state = state.clone();
        tokio::spawn(async move {
            let run = transport.run(inbound_tx, cancel.clone());
            tokio::pin!(run);
            loop {
                tokio::select! {
                    message = inbound_rx.recv() => match message {
                        Some(message) => {
                            let service = Arc::clone(&service);
                            let ctx = Arc::clone(&ctx);
                            let state = loop_state.clone();
                            tokio::spawn(async move {
                                service.accept_inbound(&state, &ctx, message).await;
                            });
                        }
                        // The run future owns the sender; when it is gone the
                        // connection is over either way.
                        None => break,
                    },
                    result = &mut run => {
                        let mut cell = ctx.runtime.lock().unwrap_or_else(PoisonError::into_inner);
                        match result {
                            // Cancelled stop (reconcile or shutdown) is not an error.
                            Ok(()) if cancel.is_cancelled() => cell.state = ImChannelState::Disabled,
                            Ok(()) => cell.state = ImChannelState::Error,
                            Err(err) => {
                                cell.state = ImChannelState::Error;
                                cell.last_error = Some(err.message.clone());
                            }
                        }
                        drop(cell);
                        break;
                    }
                }
            }
        });
        tracing::info!(channel = %channel.id, kind = ?channel.kind, "im channel started");
        Ok(())
    }

    /// Stop one channel immediately (used by DELETE before the store cascade).
    pub fn stop_channel(&self, id: &ImChannelId) {
        if let Some(handle) = self.lock().channels.remove(id) {
            handle.cancel.cancel();
        }
    }

    fn stop_all(&self) {
        let handles: Vec<ChannelHandle> = self.lock().channels.drain().map(|(_, h)| h).collect();
        for handle in handles {
            handle.cancel.cancel();
        }
    }

    /// Fold cursor advances into `im.json`, throttled per channel.
    fn persist_cursors(&self, records: &[ImChannelRecord]) {
        let mut state = self.lock();
        for record in records {
            let Some(handle) = state.channels.get_mut(&record.channel.id) else {
                continue;
            };
            let cursor = handle.transport.cursor();
            if cursor == handle.cursor_baseline {
                continue;
            }
            // The first advance past the persisted baseline persists
            // immediately (a restart must not replay it); rapid further
            // advances coalesce within the throttle window.
            let due = handle.cursor_messages_since == 0
                || handle.cursor_last_persist.elapsed() >= CURSOR_PERSIST_INTERVAL
                || handle.cursor_messages_since >= CURSOR_PERSIST_MAX_MESSAGES;
            handle.cursor_messages_since += 1;
            if !due {
                continue;
            }
            // Re-read before writing: a PATCH may have changed other fields
            // since this sweep started, and the cursor must not clobber them.
            match self.store.get_channel(&record.channel.id) {
                Ok(mut current) => {
                    current.cursor = cursor.clone();
                    match self.store.replace_channel(current) {
                        Ok(updated) => {
                            handle.cursor_baseline = updated.cursor;
                            handle.cursor_messages_since = 0;
                            handle.cursor_last_persist = Instant::now();
                        }
                        Err(err) => tracing::warn!(
                            channel = %record.channel.id,
                            error = %err.message,
                            "cannot persist im channel cursor"
                        ),
                    }
                }
                Err(err) => tracing::warn!(
                    channel = %record.channel.id,
                    error = %err.message,
                    "cannot re-read im channel for cursor persistence"
                ),
            }
        }
    }

    /// Mark a send/receive failure on the live runtime cell.
    pub fn record_error(&self, id: &ImChannelId, message: String) {
        if let Some(handle) = self.lock().channels.get(id) {
            let mut cell = handle
                .runtime
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            cell.last_error = Some(message);
        }
    }

    /// Record inbound activity on the live runtime cell.
    pub fn record_inbound(&self, id: &ImChannelId) {
        if let Some(handle) = self.lock().channels.get(id) {
            let mut cell = handle
                .runtime
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            cell.last_inbound_at = Some(chrono::Utc::now());
        }
    }

    /// Deduplicate one platform message id for a channel.
    pub fn is_duplicate(&self, channel_id: &ImChannelId, message_id: &str) -> bool {
        let mut state = self.lock();
        let set = state.dedup.entry(channel_id.clone()).or_default();
        !set.insert(message_id)
    }

    fn self_arc(self: &Arc<Self>) -> Arc<Self> {
        Arc::clone(self)
    }
}
/// Config fingerprint for reconcile decisions: any user-visible change (fields
/// or credentials) rebuilds the connection; serialization covers them all.
fn fingerprint(channel: &ImChannel) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(channel)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}
