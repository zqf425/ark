//! Integration tests for the IM orchestration (fake transport, real pipeline).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ark_common::ItemId;
use ark_config::{ConfigStore, RuntimeConfig};
use ark_contract::{
    ImChannel, ImChannelId, ImChannelKind, ImChannelPolicy, ImChannelRuntime, ImCredentials,
    ImSettings, ItemKind, ItemStatus, ThreadItem,
};
use ark_im::{ImTransport, InboundMessage};
use ark_obs::UsageRecorder;
use ark_ports::{CancelToken, ImChannelRecord, ImStore, SessionStore, ToolRegistryPort};
use ark_store::{FileSessionStore, ImFileStore, StoreConfig};
use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::mpsc;

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

/// Records `send_text` calls, replays scripted inbound messages, and reports a
/// shared cursor the test can advance.
struct FakeTransport {
    scripted: Vec<InboundMessage>,
    sent: Arc<Mutex<Vec<String>>>,
    cursor: Arc<Mutex<Option<String>>>,
}

#[async_trait]
impl ImTransport for FakeTransport {
    fn kind(&self) -> ImChannelKind {
        ImChannelKind::Telegram
    }

    async fn run(
        &self,
        inbound: mpsc::Sender<InboundMessage>,
        shutdown: CancelToken,
    ) -> ark_common::Result<()> {
        for message in &self.scripted {
            let _ = inbound.send(message.clone()).await;
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

    fn cursor(&self) -> Option<String> {
        self.cursor.lock().unwrap().clone()
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

fn service(store: Arc<dyn ImStore>, sent: Arc<Mutex<Vec<String>>>) -> Arc<ImService> {
    ImService::with_factory(
        store,
        reqwest::Client::new(),
        Box::new(move |_client, _kind, _credentials, _cursor| {
            Ok(Arc::new(FakeTransport {
                scripted: Vec::new(),
                sent: Arc::clone(&sent),
                cursor: Arc::new(Mutex::new(None)),
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
            cursor: Arc::new(Mutex::new(None)),
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
async fn sync_persists_an_advanced_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner as Arc<dyn TurnRunner>);
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));

    im.insert_channel(ImChannelRecord {
        channel: channel("im_1", vec![]),
        cursor: Some("10".to_string()),
    })
    .unwrap();

    // The factory asserts the resume cursor and hands the transport a shared
    // cursor cell the test can advance afterwards.
    let live_cursor = Arc::new(Mutex::new(Some("10".to_string())));
    let factory_cursor = Arc::clone(&live_cursor);
    let service = {
        let sent = Arc::clone(&sent);
        ImService::with_factory(
            Arc::clone(&im),
            reqwest::Client::new(),
            Box::new(move |_client, _kind, _credentials, cursor| {
                assert_eq!(
                    cursor.as_deref(),
                    Some("10"),
                    "the transport resumes from the stored cursor"
                );
                Ok(Arc::new(FakeTransport {
                    scripted: Vec::new(),
                    sent: Arc::clone(&sent),
                    cursor: Arc::clone(&factory_cursor),
                }) as Arc<dyn ImTransport>)
            }),
        )
    };

    *live_cursor.lock().unwrap() = Some("42".to_string());
    service.sync(&app_state).await;
    assert_eq!(
        im.get_channel(&ImChannelId("im_1".into()))
            .unwrap()
            .cursor
            .as_deref(),
        Some("42"),
        "the sync sweep persists the transport's advanced cursor"
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
        runner,
        dir.path().to_path_buf(),
        "test",
    );
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
        ark_contract::ImChannelState::Disabled,
        "master switch off means no connection; the store stays editable"
    );
}
