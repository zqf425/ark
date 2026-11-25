use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};

use ark_contract::ImChannelState;
use ark_im::{ImTransport, InboundMessage, InboundSender};
use ark_ports::ImStore;
use tokio::sync::Notify;

use super::*;

fn service_with_transport(
    store: Arc<dyn ImStore>,
    transport: Arc<dyn ImTransport>,
) -> Arc<ImService> {
    ImService::with_factory(
        store,
        reqwest::Client::new(),
        Box::new(move |_client, _kind, _credentials, _cursor| Ok(Arc::clone(&transport))),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_failure_admits_a_delivery_persisted_before_teardown() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let im_dir = tempfile::tempdir().unwrap();
    let inner: Arc<dyn ImStore> = Arc::new(ImFileStore::open(im_dir.path()).unwrap());
    let attempted = Arc::new(Notify::new());
    let persisted = Arc::new(Notify::new());
    let transport_ready = Arc::new(Notify::new());
    let (release_tx, release_rx) = mpsc::channel();
    let im: Arc<dyn ImStore> = Arc::new(HookStore {
        inner,
        fail_binding: false,
        attempted,
        persisted: Some(Arc::clone(&persisted)),
        persist_release: Some(Arc::new(Mutex::new(release_rx))),
    });
    let sent = Arc::new(Mutex::new(Vec::new()));
    let transport = Arc::new(FakeTransport {
        scripted: vec![message("100", "7", "persisted before disconnect")],
        sent: Arc::clone(&sent),
        fail_after_persist: Some(Arc::clone(&transport_ready)),
    });
    let service = service_with_transport(Arc::clone(&im), transport);
    let channel = channel("im_1", vec!["100".to_string()]);
    im.insert_channel(ImChannelRecord {
        channel: channel.clone(),
        cursor: None,
    })
    .unwrap();

    service.start_channel(&app_state, &channel).await.unwrap();
    persisted.notified().await;
    transport_ready.notify_one();
    // Let the transport outcome be applied before allowing persistence to
    // offer the delivery. The broken supervisor exits at this point.
    let _ = wait_for(
        || service.channel_runtime(&channel.id).state == ImChannelState::Error,
        1,
    )
    .await;
    release_tx.send(()).unwrap();

    assert!(
        wait_for(|| im.list_inbox().unwrap().is_empty(), 10).await,
        "a delivery persisted before transport failure must be admitted and acked"
    );
    assert_eq!(runner.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        im.get_channel(&channel.id).unwrap().cursor.as_deref(),
        Some("8")
    );
    assert!(
        wait_for(
            || service.channel_runtime(&channel.id).state == ImChannelState::Error,
            10
        )
        .await
    );
    assert!(service
        .channel_runtime(&channel.id)
        .last_error
        .is_some_and(|error| error.contains("injected transport failure")));
}

#[tokio::test]
async fn different_durable_ids_for_one_active_platform_message_run_once() {
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
    let ctx = context(channel.clone(), &sent);
    let (sender, mut receiver) = InboundSender::new(Arc::clone(&im), channel.id.clone());
    let send = tokio::spawn(async move {
        sender
            .send(message("100", "7", "original"), Some("8".into()))
            .await
    });
    service
        .accept_delivery(&app_state, &ctx, receiver.recv().await.unwrap())
        .await;
    send.await.unwrap().unwrap();
    runner.entered.notified().await;

    let mut duplicate = message("100", "7", "gateway redelivery");
    duplicate.channel_id = channel.id.clone();
    let duplicate_id = im.persist_inbound(duplicate, Some("9".into())).unwrap();
    im.queue_inbound(&duplicate_id, Some("9".into())).unwrap();
    let entry = im
        .list_inbox()
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == duplicate_id)
        .unwrap();
    service
        .accept_delivery(&app_state, &ctx, InboundDelivery::replay(entry))
        .await;

    assert_eq!(
        im.list_inbox().unwrap().len(),
        1,
        "the duplicate durable id must be discarded while the original is active"
    );
    runner.release.notify_one();
    assert!(wait_for(|| im.list_inbox().unwrap().is_empty(), 10).await);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
}

struct FirstBlockingRunner {
    calls: AtomicUsize,
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl crate::TurnRunner for FirstBlockingRunner {
    async fn run_turn(&self, _job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(crate::TurnEnd::Completed)
    }
}

#[tokio::test]
async fn a_full_replay_queue_is_retried_after_capacity_frees() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(FirstBlockingRunner {
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let app_state = state(dir.path(), runner.clone() as Arc<dyn crate::TurnRunner>);
    let (_im_dir, im) = im_store();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(Arc::clone(&im), Arc::clone(&sent));
    let channel = channel("im_1", vec!["100".to_string()]);
    im.insert_channel(ImChannelRecord {
        channel: channel.clone(),
        cursor: None,
    })
    .unwrap();
    let ctx = context(channel.clone(), &sent);
    service
        .accept_inbound(&app_state, &ctx, message("100", "running", "first"))
        .await;
    runner.entered.notified().await;

    let key = (channel.id.clone(), "100".to_string());
    for index in 0..crate::service::im::CONVERSATION_QUEUE_CAP {
        service.conversations.enqueue(
            key.clone(),
            message("100", &format!("waiting-{index}"), "already waiting"),
            None,
        );
    }
    let mut inbound = message("100", "replay", "retry without another restart");
    inbound.channel_id = channel.id.clone();
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
    assert!(
        wait_for(
            || im
                .list_inbox()
                .unwrap()
                .iter()
                .any(|entry| entry.id == id && entry.queued),
            1,
        )
        .await,
        "replay reaches its durable queue commit before waiting for capacity"
    );

    runner.release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), replay)
        .await
        .expect("replay admission settles after capacity frees")
        .expect("replay task");
    assert!(wait_for(|| im.list_inbox().unwrap().is_empty(), 10).await);
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        (crate::service::im::CONVERSATION_QUEUE_CAP + 2),
        "the retained replay must run in this process after earlier queued work"
    );
    assert!(sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn live_queue_overflow_discards_and_acknowledges_the_durable_record() {
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
    let ctx = context(channel.clone(), &sent);
    let key = (channel.id.clone(), "100".to_string());
    for index in 0..crate::service::im::CONVERSATION_QUEUE_CAP {
        service.conversations.enqueue(
            key.clone(),
            message("100", &format!("queued-{index}"), "already waiting"),
            None,
        );
    }
    let (sender, mut receiver) = InboundSender::new(Arc::clone(&im), channel.id.clone());
    let send = tokio::spawn(async move {
        sender
            .send(message("100", "7", "overflow"), Some("8".into()))
            .await
    });
    let delivery = receiver.recv().await.unwrap();
    service.accept_delivery(&app_state, &ctx, delivery).await;
    send.await.unwrap().unwrap();

    assert!(im.list_inbox().unwrap().is_empty());
    assert_eq!(
        im.get_channel(&channel.id).unwrap().cursor.as_deref(),
        Some("8")
    );
    assert_eq!(runner.0.load(Ordering::SeqCst), 0);
    assert_eq!(sent.lock().unwrap().len(), 1, "one busy reply");
}

#[tokio::test]
async fn reopened_store_replays_queued_and_pending_batch_entries() {
    let dir = tempfile::tempdir().unwrap();
    let im_dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
    let channel = channel("im_1", vec!["100".to_string()]);
    let store = ImFileStore::open(im_dir.path()).unwrap();
    store
        .insert_channel(ImChannelRecord {
            channel: channel.clone(),
            cursor: None,
        })
        .unwrap();
    let mut first: InboundMessage = message("100", "7", "queued before crash");
    first.channel_id = channel.id.clone();
    let mut second: InboundMessage = message("100", "8", "pending in same batch");
    second.channel_id = channel.id.clone();
    let ids = store
        .persist_inbound_batch(vec![first, second], Some("9".into()))
        .unwrap();
    store.queue_inbound(&ids[0], Some("9".into())).unwrap();
    assert_eq!(
        store.get_channel(&channel.id).unwrap().cursor.as_deref(),
        Some("9")
    );
    drop(store);

    let reopened: Arc<dyn ImStore> = Arc::new(ImFileStore::open(im_dir.path()).unwrap());
    let app_state = state(dir.path(), runner.clone() as Arc<dyn TurnRunner>);
    let sent = Arc::new(Mutex::new(Vec::new()));
    let service = service(Arc::clone(&reopened), Arc::clone(&sent));
    service.sync(&app_state).await;

    assert!(wait_for(|| runner.0.load(Ordering::SeqCst) == 2, 10).await);
    assert!(wait_for(|| reopened.list_inbox().unwrap().is_empty(), 10).await);
    assert_eq!(
        reopened.get_channel(&channel.id).unwrap().cursor.as_deref(),
        Some("9")
    );
    service.stop_channel(&channel.id);
}
