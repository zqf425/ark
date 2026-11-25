//! Integration tests for the IM orchestration (fake transport, real pipeline).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ark_common::{ArkError, ArkErrorCode, ItemId};
use ark_config::{ConfigStore, RuntimeConfig};
use ark_contract::{
    ImBinding, ImChannel, ImChannelId, ImChannelKind, ImChannelPolicy, ImChannelRuntime,
    ImCredentials, ImSettings, ItemKind, ItemStatus, ThreadItem,
};
use ark_im::{ImTransport, InboundDelivery, InboundMessage, InboundSender};
use ark_obs::UsageRecorder;
use ark_ports::{CancelToken, ImChannelRecord, ImInboxId, ImStore, SessionStore, ToolRegistryPort};
use ark_store::{FileSessionStore, ImFileStore, StoreConfig};
use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Notify;

use crate::state::{AppState, TurnEnd, TurnJob, TurnRunner};
use crate::ImService;

use crate::service::im_inbound::ChannelContext;

struct EmptyTools;

impl ToolRegistryPort for EmptyTools {
    fn names(&self) -> Vec<String> {
        Vec::new()
    }

    fn schemas(&self) -> Vec<serde_json::Value> {
        Vec::new()
    }

    fn version(&self) -> u64 {
        0
    }
}

/// Records `send_text` calls and replays scripted inbound messages through the
/// same durable delivery boundary as a real platform adapter.
struct FakeTransport {
    scripted: Vec<InboundMessage>,
    sent: Arc<Mutex<Vec<String>>>,
    fail_after_persist: Option<Arc<Notify>>,
}

#[async_trait]
impl ImTransport for FakeTransport {
    fn kind(&self) -> ImChannelKind {
        ImChannelKind::Telegram
    }

    async fn run(&self, inbound: InboundSender, shutdown: CancelToken) -> ark_common::Result<()> {
        if let Some(persisted) = &self.fail_after_persist {
            let sender = inbound.clone();
            let message = self.scripted.first().cloned().expect("scripted message");
            tokio::spawn(async move {
                let _ = sender.send(message, Some("8".into())).await;
            });
            persisted.notified().await;
            return Err(ArkError::new(
                ArkErrorCode::RuntimeUnavailable,
                "injected transport failure after persistence",
            ));
        }
        for message in &self.scripted {
            if inbound.send(message.clone(), None).await.is_err() {
                return Ok(());
            }
        }
        shutdown.cancelled().await;
        Ok(())
    }

    async fn send_text(&self, chat_id: &str, text: &str) -> ark_common::Result<()> {
        self.sent.lock().unwrap().push(format!("{chat_id}:{text}"));
        Ok(())
    }

    async fn verify(&self) -> ark_common::Result<String> {
        Ok("@fake".to_string())
    }
}

/// A runner that appends one assistant item so the reply path has something to
/// project, then completes.
struct ReplyRunner {
    store: Arc<dyn SessionStore>,
    marker: &'static str,
    calls: AtomicUsize,
}

#[async_trait]
impl TurnRunner for ReplyRunner {
    async fn run_turn(&self, job: TurnJob) -> ark_common::Result<TurnEnd> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let item = ThreadItem {
            item_id: ItemId::new(),
            turn_id: job.turn_id,
            kind: ItemKind::AssistantMessage,
            status: ItemStatus::Completed,
            created_at: Utc::now(),
            text: Some(self.marker.to_string()),
            tool: None,
            detail: None,
            marker: None,
            attachments: None,
            model: None,
        };
        self.store
            .append_event(
                job.thread_id,
                ark_contract::RuntimeEvent::ItemCompleted {
                    seq: 0,
                    thread_id: job.thread_id,
                    turn_id: job.turn_id,
                    timestamp: Utc::now(),
                    item,
                },
            )
            .await?;
        Ok(TurnEnd::Completed)
    }
}

struct BlockingRunner {
    calls: AtomicUsize,
    entered: Notify,
    release: Notify,
}

impl Default for BlockingRunner {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            entered: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[async_trait]
impl TurnRunner for BlockingRunner {
    async fn run_turn(&self, _job: TurnJob) -> ark_common::Result<TurnEnd> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        Ok(TurnEnd::Completed)
    }
}

struct CountingRunner(AtomicUsize);

#[async_trait]
impl TurnRunner for CountingRunner {
    async fn run_turn(&self, _job: TurnJob) -> ark_common::Result<TurnEnd> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(TurnEnd::Completed)
    }
}

fn channel(id: &str, whitelist: Vec<String>) -> ImChannel {
    ImChannel {
        id: ImChannelId(id.to_string()),
        kind: ImChannelKind::Telegram,
        title: "tg".to_string(),
        enabled: true,
        credentials: Some(ImCredentials::Telegram {
            bot_token: "tok".to_string(),
            bot_username: None,
        }),
        policy: ImChannelPolicy {
            allowed_chat_ids: whitelist,
            require_mention: true,
        },
        workspace: None,
        model: None,
        provider_id: None,
        autonomy: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn message(chat_id: &str, message_id: &str, text: &str) -> InboundMessage {
    InboundMessage {
        channel_id: ImChannelId(String::new()),
        chat_id: chat_id.to_string(),
        sender_id: "sender-1".to_string(),
        sender_name: "Sender".to_string(),
        message_id: message_id.to_string(),
        text: text.to_string(),
        is_dm: true,
        is_mention: false,
    }
}

fn state(data_dir: &std::path::Path, runner: Arc<dyn TurnRunner>) -> AppState {
    let sessions =
        Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("session store"));
    state_with_store(data_dir, sessions, runner)
}

/// Same fixture with a caller-owned session store (the reply test shares one
/// store between the runner and the state; the data directory allows exactly
/// one store instance).
fn state_with_store(
    data_dir: &std::path::Path,
    sessions: Arc<FileSessionStore>,
    runner: Arc<dyn TurnRunner>,
) -> AppState {
    let store: Arc<dyn SessionStore> = sessions;
    // The IM master switch defaults to off; the connection tests turn it on.
    let config = RuntimeConfig {
        im: ImSettings { enabled: true },
        ..RuntimeConfig::default()
    };
    AppState::new(
        store,
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(config)),
        Arc::new(UsageRecorder::new()),
        runner,
        data_dir.to_path_buf(),
        "test",
    )
}

fn im_store() -> (tempfile::TempDir, Arc<dyn ImStore>) {
    let dir = tempfile::tempdir().expect("im store dir");
    let store = ImFileStore::open(dir.path()).expect("im store");
    (dir, Arc::new(store))
}

struct HookStore {
    inner: Arc<dyn ImStore>,
    fail_binding: bool,
    attempted: Arc<Notify>,
    persisted: Option<Arc<Notify>>,
    persist_release: Option<Arc<Mutex<std::sync::mpsc::Receiver<()>>>>,
}

impl ImStore for HookStore {
    fn list_channels(&self) -> ark_common::Result<Vec<ImChannelRecord>> {
        self.inner.list_channels()
    }

    fn get_channel(&self, id: &ImChannelId) -> ark_common::Result<ImChannelRecord> {
        self.inner.get_channel(id)
    }

    fn insert_channel(&self, record: ImChannelRecord) -> ark_common::Result<ImChannelRecord> {
        self.inner.insert_channel(record)
    }

    fn replace_channel(&self, record: ImChannelRecord) -> ark_common::Result<ImChannelRecord> {
        self.inner.replace_channel(record)
    }

    fn delete_channel(&self, id: &ImChannelId) -> ark_common::Result<()> {
        self.inner.delete_channel(id)
    }

    fn list_bindings(
        &self,
        channel_id: Option<&ImChannelId>,
    ) -> ark_common::Result<Vec<ImBinding>> {
        self.inner.list_bindings(channel_id)
    }

    fn get_binding(
        &self,
        channel_id: &ImChannelId,
        chat_id: &str,
    ) -> ark_common::Result<Option<ImBinding>> {
        self.attempted.notify_one();
        if self.fail_binding {
            return Err(ArkError::new(
                ArkErrorCode::StoreFailure,
                format!("injected binding read failure for {channel_id}/{chat_id}"),
            ));
        }
        self.inner.get_binding(channel_id, chat_id)
    }

    fn set_binding(&self, binding: ImBinding) -> ark_common::Result<()> {
        self.inner.set_binding(binding)
    }

    fn delete_binding(&self, channel_id: &ImChannelId, chat_id: &str) -> ark_common::Result<()> {
        self.inner.delete_binding(channel_id, chat_id)
    }

    fn persist_inbound(
        &self,
        message: InboundMessage,
        next_cursor: Option<String>,
    ) -> ark_common::Result<ImInboxId> {
        let id = self.inner.persist_inbound(message, next_cursor)?;
        if let Some(persisted) = &self.persisted {
            persisted.notify_one();
        }
        if let Some(release) = &self.persist_release {
            let _ = release.lock().unwrap().recv();
        }
        Ok(id)
    }

    fn persist_inbound_batch(
        &self,
        messages: Vec<InboundMessage>,
        next_cursor: Option<String>,
    ) -> ark_common::Result<Vec<ImInboxId>> {
        let ids = self.inner.persist_inbound_batch(messages, next_cursor)?;
        if let Some(persisted) = &self.persisted {
            persisted.notify_one();
        }
        if let Some(release) = &self.persist_release {
            let _ = release.lock().unwrap().recv();
        }
        Ok(ids)
    }

    fn list_inbox(&self) -> ark_common::Result<Vec<ark_ports::ImInboxEntry>> {
        self.inner.list_inbox()
    }

    fn advance_channel_cursor(
        &self,
        channel_id: &ImChannelId,
        next: Option<String>,
    ) -> ark_common::Result<()> {
        self.inner.advance_channel_cursor(channel_id, next)
    }

    fn queue_inbound(&self, id: &ImInboxId, next_cursor: Option<String>) -> ark_common::Result<()> {
        self.inner.queue_inbound(id, next_cursor)
    }

    fn ack_inbound(&self, id: &ImInboxId) -> ark_common::Result<()> {
        self.inner.ack_inbound(id)
    }

    fn discard_inbound(
        &self,
        id: &ImInboxId,
        next_cursor: Option<String>,
    ) -> ark_common::Result<()> {
        self.inner.discard_inbound(id, next_cursor)
    }
}

fn service(store: Arc<dyn ImStore>, sent: Arc<Mutex<Vec<String>>>) -> Arc<ImService> {
    ImService::with_factory(
        store,
        reqwest::Client::new(),
        Box::new(move |_client, _kind, _credentials, _cursor| {
            Ok(Arc::new(FakeTransport {
                scripted: Vec::new(),
                sent: Arc::clone(&sent),
                fail_after_persist: None,
            }) as Arc<dyn ImTransport>)
        }),
    )
}

fn context(channel: ImChannel, sent: &Arc<Mutex<Vec<String>>>) -> Arc<ChannelContext> {
    Arc::new(ChannelContext {
        channel,
        transport: Arc::new(FakeTransport {
            scripted: Vec::new(),
            sent: Arc::clone(sent),
            fail_after_persist: None,
        }),
        runtime: Arc::new(Mutex::new(ImChannelRuntime::default())),
    })
}

async fn wait_for(predicate: impl Fn() -> bool, seconds: u64) -> bool {
    for _ in 0..(seconds * 10) {
        if predicate() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    predicate()
}

#[tokio::test]
async fn same_message_id_in_different_chats_runs_both_turns() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(im, Arc::clone(&sent));
    let channel = channel("im_1", vec!["100".to_string(), "200".to_string()]);
    let ctx = context(channel, &sent);

    service
        .accept_inbound(&app_state, &ctx, message("100", "7", "first"))
        .await;
    service
        .accept_inbound(&app_state, &ctx, message("200", "7", "second"))
        .await;

    assert!(
        wait_for(|| runner.0.load(Ordering::SeqCst) == 2, 10).await,
        "identical platform ids in different chats must both be dispatched"
    );
}

#[tokio::test]
async fn an_unknown_chat_gets_a_whitelist_hint_and_no_turn() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(im, Arc::clone(&sent));

    let channel = channel("im_1", Vec::new());
    service
        .store()
        .insert_channel(ImChannelRecord {
            channel: channel.clone(),
            cursor: None,
        })
        .unwrap();
    let ctx = context(channel, &sent);

    service
        .accept_inbound(&app_state, &ctx, message("999", "m1", "hello"))
        .await;

    let replies = sent.lock().unwrap().join(" | ");
    assert!(
        replies.contains("999"),
        "hint must name the chat id: {replies}"
    );
    assert!(
        replies.contains("白名单"),
        "hint must explain the whitelist"
    );
    assert_eq!(runner.0.load(Ordering::SeqCst), 0, "no turn for a stranger");
    assert!(service
        .store()
        .get_binding(&ImChannelId("im_1".into()), "999")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_whitelisted_message_creates_a_thread_runs_a_turn_and_replies() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = Arc::new(FileSessionStore::open(StoreConfig::new(dir.path())).unwrap());
    let store: Arc<dyn SessionStore> = sessions.clone();
    let runner = Arc::new(ReplyRunner {
        store: Arc::clone(&store),
        marker: "hello from ark",
        calls: AtomicUsize::new(0),
    });
    let app_state = state_with_store(dir.path(), sessions, runner.clone() as Arc<dyn TurnRunner>);
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(im, Arc::clone(&sent));

    let channel = channel("im_1", vec!["100".to_string()]);
    let ctx = context(channel, &sent);

    service
        .accept_inbound(&app_state, &ctx, message("100", "m1", "hi there"))
        .await;

    assert!(
        wait_for(
            || sent
                .lock()
                .unwrap()
                .iter()
                .any(|reply| reply.contains("hello from ark")),
            10
        )
        .await,
        "reply must eventually carry the runner's text: {:?}",
        sent.lock().unwrap()
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);

    let binding = service
        .store()
        .get_binding(&ImChannelId("im_1".into()), "100")
        .unwrap()
        .expect("binding persisted");
    let meta = app_state
        .store
        .get_thread(binding.thread_id)
        .await
        .expect("thread exists");
    assert_eq!(meta.runtime.autonomy.as_deref(), Some("unattended"));
}

#[tokio::test]
async fn duplicate_message_ids_run_one_turn() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(im, Arc::clone(&sent));

    let channel = channel("im_1", vec!["100".to_string()]);
    let ctx = context(channel, &sent);

    service
        .accept_inbound(&app_state, &ctx, message("100", "m1", "once"))
        .await;
    service
        .accept_inbound(&app_state, &ctx, message("100", "m1", "once"))
        .await;

    assert!(
        wait_for(|| runner.0.load(Ordering::SeqCst) == 1, 10).await,
        "exactly one turn"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        runner.0.load(Ordering::SeqCst),
        1,
        "the duplicate must never start a second turn"
    );
}

#[tokio::test]
async fn an_outside_chat_is_silently_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(im, Arc::clone(&sent));

    let channel = channel("im_1", vec!["100".to_string()]);
    let ctx = context(channel, &sent);

    service
        .accept_inbound(&app_state, &ctx, message("200", "m1", "sneak"))
        .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(sent.lock().unwrap().is_empty(), "no reply to outsiders");
    assert_eq!(runner.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sync_replays_persisted_inbox_before_new_platform_messages() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let im_dir = tempfile::tempdir().unwrap();
    let channel = channel("im_1", vec!["100".to_string()]);
    std::fs::write(
        im_dir.path().join("im.json"),
        serde_json::json!({
            "channels": [{"channel": channel, "cursor": "7"}],
            "bindings": [],
            "inbox": [{
                "id": "in_1",
                "queued": true,
                "next_cursor": "8",
                "message": {
                    "channel_id": "im_1",
                    "chat_id": "100",
                    "sender_id": "sender-1",
                    "sender_name": "Sender",
                    "message_id": "7",
                    "text": "replay me",
                    "is_dm": true,
                    "is_mention": false
                }
            }]
        })
        .to_string(),
    )
    .unwrap();
    let im: Arc<dyn ImStore> = Arc::new(ImFileStore::open(im_dir.path()).unwrap());
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(Arc::clone(&im), Arc::clone(&sent));

    service.sync(&app_state).await;

    assert!(
        wait_for(|| runner.0.load(Ordering::SeqCst) == 1, 10).await,
        "an inbox entry durable before the crash must be replayed"
    );
    assert!(
        wait_for(|| im.list_inbox().unwrap().is_empty(), 10).await,
        "completed replay must acknowledge and remove the durable entry"
    );
}

#[tokio::test]
async fn sync_preserves_inbox_for_a_missing_channel() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let im_dir = tempfile::tempdir().unwrap();
    let mut orphan = message("100", "7", "orphan");
    orphan.channel_id = ImChannelId("im_missing".into());
    // Write the orphan directly: normal persistence rejects absent channels.
    std::fs::write(
        im_dir.path().join("im.json"),
        serde_json::json!({
            "channels": [],
            "bindings": [],
            "inbox": [{
                "id": "in_orphan",
                "queued": false,
                "message": orphan
            }]
        })
        .to_string(),
    )
    .unwrap();
    let im: Arc<dyn ImStore> = Arc::new(ImFileStore::open(im_dir.path()).unwrap());
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(Arc::clone(&im), sent);

    service.sync(&app_state).await;

    assert_eq!(runner.0.load(Ordering::SeqCst), 0);
    assert_eq!(im.list_inbox().unwrap().len(), 1);
}

#[tokio::test]
async fn sync_starts_enabled_channels_and_stops_deleted_ones() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner as Arc<dyn TurnRunner>);
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(im, Arc::clone(&sent));

    service
        .store()
        .insert_channel(ImChannelRecord {
            channel: channel("im_1", vec![]),
            cursor: None,
        })
        .unwrap();
    service.sync(&app_state).await;
    assert_eq!(
        service.channel_runtime(&ImChannelId("im_1".into())).state,
        ark_contract::ImChannelState::Connecting,
        "the fake transport parks in run() until cancelled"
    );

    service
        .store()
        .delete_channel(&ImChannelId("im_1".into()))
        .unwrap();
    service.sync(&app_state).await;
    assert_eq!(
        service.channel_runtime(&ImChannelId("im_1".into())).state,
        ark_contract::ImChannelState::Disabled,
        "a deleted channel loses its handle"
    );
}

#[tokio::test]
async fn delivery_persists_a_whole_batch_before_offering_its_first_message() {
    let (_im_dir, im) = im_store();
    im.insert_channel(ImChannelRecord {
        channel: channel("im_1", vec!["100".to_string()]),
        cursor: None,
    })
    .unwrap();
    let (sender, mut receiver) = InboundSender::new(Arc::clone(&im), ImChannelId("im_1".into()));
    let send = tokio::spawn(async move {
        sender
            .send_batch(
                vec![message("100", "7", "first"), message("100", "8", "second")],
                Some("9".into()),
            )
            .await
    });

    let first = receiver.recv().await.unwrap();
    assert_eq!(im.list_inbox().unwrap().len(), 2);
    assert!(im.list_inbox().unwrap().iter().all(|entry| !entry.queued));
    assert_eq!(
        im.get_channel(&first.message.channel_id).unwrap().cursor,
        None
    );
    first.acknowledge();
    receiver.recv().await.unwrap().acknowledge();
    send.await.unwrap().unwrap();
}

#[tokio::test]
async fn unfinished_processing_retains_the_durable_inbox_entry() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let im_dir = tempfile::tempdir().unwrap();
    let inner: Arc<dyn ImStore> = Arc::new(ImFileStore::open(im_dir.path()).unwrap());
    let attempted = Arc::new(Notify::new());
    let im: Arc<dyn ImStore> = Arc::new(HookStore {
        inner: Arc::clone(&inner),
        fail_binding: true,
        attempted: Arc::clone(&attempted),
        persisted: None,
        persist_release: None,
    });
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(Arc::clone(&im), Arc::clone(&sent));
    let channel = channel("im_1", vec!["100".to_string()]);
    im.insert_channel(ImChannelRecord {
        channel: channel.clone(),
        cursor: None,
    })
    .unwrap();
    let ctx = context(channel, &sent);
    let mut inbound = message("100", "7", "retry after recovery");
    inbound.channel_id = ImChannelId("im_1".into());
    let id = im.persist_inbound(inbound, Some("8".into())).unwrap();
    let entry = im
        .list_inbox()
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap();

    service
        .accept_delivery(&app_state, &ctx, InboundDelivery::replay(entry))
        .await;
    attempted.notified().await;

    assert_eq!(runner.0.load(Ordering::SeqCst), 0, "no turn can start");
    let entries = im.list_inbox().unwrap();
    assert_eq!(entries.len(), 1, "unfinished work must remain replayable");
    assert!(entries[0].queued, "queue admission itself did commit");
    assert_eq!(
        im.get_channel(&ImChannelId("im_1".into()))
            .unwrap()
            .cursor
            .as_deref(),
        Some("8")
    );
}

#[tokio::test]
async fn queued_replay_is_not_discarded_by_live_message_dedup() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(Arc::clone(&im), Arc::clone(&sent));
    let channel = channel("im_1", vec!["100".to_string()]);
    im.insert_channel(ImChannelRecord {
        channel: channel.clone(),
        cursor: None,
    })
    .unwrap();
    let ctx = context(channel, &sent);
    let mut inbound = message("100", "7", "survives a lost memory queue");
    inbound.channel_id = ImChannelId("im_1".into());
    let id = im
        .persist_inbound(inbound.clone(), Some("8".into()))
        .unwrap();
    im.queue_inbound(&id, Some("8".into())).unwrap();
    // Simulate a reconnect after admission: the live dedup cache still knows
    // the platform id, but the old in-memory conversation queue is gone.
    service.mark_seen(&inbound.channel_id, &inbound.chat_id, &inbound.message_id);
    let entry = im
        .list_inbox()
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap();

    service
        .accept_delivery(&app_state, &ctx, InboundDelivery::replay(entry))
        .await;

    assert_eq!(
        im.list_inbox().unwrap().len(),
        1,
        "a durable queued id must not be mistaken for a new duplicate"
    );
    assert!(wait_for(|| runner.0.load(Ordering::SeqCst) == 1, 10).await);
    assert!(wait_for(|| im.list_inbox().unwrap().is_empty(), 10).await);
}

#[tokio::test]
async fn replaying_an_in_memory_inbox_id_does_not_run_a_second_turn() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(BlockingRunner::default());
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(Arc::clone(&im), Arc::clone(&sent));
    let channel = channel("im_1", vec!["100".to_string()]);
    im.insert_channel(ImChannelRecord {
        channel: channel.clone(),
        cursor: None,
    })
    .unwrap();
    let ctx = context(channel, &sent);
    let (sender, mut receiver) = InboundSender::new(Arc::clone(&im), ImChannelId("im_1".into()));
    let send = tokio::spawn(async move {
        sender
            .send(message("100", "7", "only once"), Some("8".into()))
            .await
    });
    let delivery = receiver.recv().await.unwrap();
    let id = delivery.id.clone();
    service.accept_delivery(&app_state, &ctx, delivery).await;
    send.await.unwrap().unwrap();
    runner.entered.notified().await;
    let entry = im
        .list_inbox()
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap();

    service
        .accept_delivery(&app_state, &ctx, InboundDelivery::replay(entry))
        .await;
    runner.release.notify_one();

    assert!(wait_for(|| im.list_inbox().unwrap().is_empty(), 10).await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        1,
        "a reconnect replay must not duplicate work still owned by the live queue"
    );
}

#[tokio::test]
async fn full_conversation_queue_does_not_discard_a_durable_replay() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(Arc::clone(&im), Arc::clone(&sent));
    let channel = channel("im_1", vec!["100".to_string()]);
    im.insert_channel(ImChannelRecord {
        channel: channel.clone(),
        cursor: None,
    })
    .unwrap();
    let ctx = context(channel, &sent);
    let key = (ImChannelId("im_1".into()), "100".to_string());
    for index in 0..crate::service::im::CONVERSATION_QUEUE_CAP {
        service.conversations.enqueue(
            key.clone(),
            message("100", &format!("queued-{index}"), "already waiting"),
            None,
        );
    }
    let mut inbound = message("100", "7", "must remain durable");
    inbound.channel_id = ImChannelId("im_1".into());
    let id = im.persist_inbound(inbound, Some("8".into())).unwrap();
    im.queue_inbound(&id, Some("8".into())).unwrap();
    let entry = im
        .list_inbox()
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap();

    let replay_state = app_state.clone();
    let replay_service = Arc::clone(&service);
    let replay_ctx = Arc::clone(&ctx);
    let replay = tokio::spawn(async move {
        replay_service
            .accept_delivery(&replay_state, &replay_ctx, InboundDelivery::replay(entry))
            .await;
    });
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !replay.is_finished(),
        "a full durable replay waits for capacity instead of returning without a retry"
    );
    replay.abort();
    let _ = replay.await;

    let entries = im.list_inbox().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "queue pressure must not erase durable work"
    );
    assert_eq!(entries[0].id, id);
    assert!(entries[0].queued);
    assert_eq!(runner.0.load(Ordering::SeqCst), 0);
    assert!(
        sent.lock().unwrap().is_empty(),
        "a retained replay must not send a live queue-overflow reply"
    );
}

#[tokio::test]
async fn the_master_switch_off_stops_everything() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    // Default config has `im.enabled = false` — exactly the case under test.
    let sessions = Arc::new(FileSessionStore::open(StoreConfig::new(dir.path())).unwrap());
    let store: Arc<dyn SessionStore> = sessions;
    let app_state = AppState::new(
        store,
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(RuntimeConfig::default())),
        Arc::new(UsageRecorder::new()),
        runner.clone(),
        dir.path().to_path_buf(),
        "test",
    );
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(Arc::clone(&im), Arc::clone(&sent));

    service
        .store()
        .insert_channel(ImChannelRecord {
            channel: channel("im_1", vec![]),
            cursor: None,
        })
        .unwrap();
    let mut pending = message("100", "7", "retain while disabled");
    pending.channel_id = ImChannelId("im_1".into());
    im.persist_inbound(pending, Some("8".into())).unwrap();
    service.sync(&app_state).await;
    assert_eq!(
        service.channel_runtime(&ImChannelId("im_1".into())).state,
        ark_contract::ImChannelState::Disabled,
        "master switch off means no connection; the store stays editable"
    );
    assert_eq!(runner.0.load(Ordering::SeqCst), 0);
    assert_eq!(im.list_inbox().unwrap().len(), 1);
}

#[path = "tests/reliability.rs"]
mod reliability;
