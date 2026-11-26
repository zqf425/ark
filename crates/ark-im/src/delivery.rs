//! Durable inbound delivery boundary between platform adapters and the server.
//!
//! A transport never advances its platform cursor merely because an in-process
//! send completed. [`InboundSender`] first persists every normalized message in
//! a poll batch, then waits for the server to atomically admit each one to its
//! conversation queue. The inbox entry remains durable until processing ends,
//! so a crash after queue admission still replays it on restart.

use std::sync::Arc;
use std::time::Duration;

use ark_common::{ArkError, ArkErrorCode, Result};
use ark_contract::ImChannelId;
use ark_ports::{ImInboxEntry, ImInboxId, ImStore, InboundMessage};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

/// Long enough for a queue admission and its synchronous `im.json` rewrite,
/// while still letting a wedged receiver release the transport for reconnect.
const ADMISSION_TIMEOUT: Duration = Duration::from_secs(30);

/// One persisted message plus the queue-admission acknowledgement channel.
pub struct InboundDelivery {
    pub id: ImInboxId,
    pub message: InboundMessage,
    pub next_cursor: Option<String>,
    acknowledge: Option<oneshot::Sender<()>>,
}

impl InboundDelivery {
    /// Rebuild a delivery from disk. There is no live transport waiting for an
    /// acknowledgement during startup replay.
    pub fn replay(entry: ImInboxEntry) -> Self {
        Self {
            id: entry.id,
            message: entry.message,
            next_cursor: entry.next_cursor,
            acknowledge: None,
        }
    }

    /// Whether this delivery was rebuilt from the durable inbox rather than
    /// offered by a live adapter.
    pub fn is_replay(&self) -> bool {
        self.acknowledge.is_none()
    }

    /// Report successful durable queue admission (or terminal discard).
    pub fn acknowledge(self) {
        if let Some(acknowledge) = self.acknowledge {
            let _ = acknowledge.send(());
        }
    }
}

/// Cloneable adapter-side handle. The matching receiver is owned by the
/// server's channel supervisor.
#[derive(Clone)]
pub struct InboundSender {
    store: Arc<dyn ImStore>,
    channel_id: ImChannelId,
    sender: mpsc::Sender<InboundDelivery>,
}

impl InboundSender {
    pub fn new(
        store: Arc<dyn ImStore>,
        channel_id: ImChannelId,
    ) -> (Self, mpsc::Receiver<InboundDelivery>) {
        let (sender, receiver) = mpsc::channel(32);
        (
            Self {
                store,
                channel_id,
                sender,
            },
            receiver,
        )
    }

    /// Persist one message before offering it to the server.
    pub async fn send(&self, message: InboundMessage, next_cursor: Option<String>) -> Result<()> {
        self.send_batch(vec![message], next_cursor).await
    }

    /// Persist the whole platform batch before sending its first entry.
    ///
    /// Every entry carries the batch's final cursor. Advancing it while later
    /// entries are still pending is safe because all of them are already on
    /// disk and startup replay drains pending as well as queued entries.
    pub async fn send_batch(
        &self,
        messages: Vec<InboundMessage>,
        next_cursor: Option<String>,
    ) -> Result<()> {
        let mut messages = messages;
        for message in &mut messages {
            message.channel_id = self.channel_id.clone();
        }
        let store = Arc::clone(&self.store);
        let channel_id = self.channel_id.clone();
        let expected = messages.len();
        let persisted_messages = messages.clone();
        let persisted_cursor = next_cursor.clone();
        let ids = tokio::task::spawn_blocking(move || {
            store.persist_inbound_batch(persisted_messages, persisted_cursor)
        })
        .await
        .map_err(|error| blocking_error("persist inbound batch", &channel_id, error))??;
        if ids.len() != expected {
            return Err(ArkError::new(
                ArkErrorCode::StoreFailure,
                "im store returned the wrong number of durable inbox ids",
            ));
        }
        for (id, message) in ids.into_iter().zip(messages) {
            let (acknowledge, admitted) = oneshot::channel();
            let delivery = InboundDelivery {
                id,
                message,
                next_cursor: next_cursor.clone(),
                acknowledge: Some(acknowledge),
            };
            self.deliver(delivery, admitted).await?;
        }
        Ok(())
    }

    /// Commit a cursor for a batch with no normalized text messages.
    pub async fn advance(&self, next_cursor: Option<String>) -> Result<()> {
        let store = Arc::clone(&self.store);
        let channel_id = self.channel_id.clone();
        let cursor_channel = channel_id.clone();
        tokio::task::spawn_blocking(move || store.advance_channel_cursor(&channel_id, next_cursor))
            .await
            .map_err(|error| blocking_error("advance channel cursor", &cursor_channel, error))?
    }

    async fn deliver(
        &self,
        delivery: InboundDelivery,
        admitted: oneshot::Receiver<()>,
    ) -> Result<()> {
        self.sender.send(delivery).await.map_err(|_| {
            ArkError::new(
                ArkErrorCode::RuntimeUnavailable,
                "im inbound supervisor closed",
            )
        })?;
        match timeout(ADMISSION_TIMEOUT, admitted).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(ArkError::new(
                ArkErrorCode::RuntimeUnavailable,
                "im inbound admission acknowledgement closed",
            )),
            Err(_) => Err(ArkError::new(
                ArkErrorCode::RuntimeUnavailable,
                "timed out waiting for durable im inbound admission",
            )),
        }
    }
}

fn blocking_error(
    operation: &str,
    channel_id: &ImChannelId,
    error: tokio::task::JoinError,
) -> ArkError {
    let code = if error.is_cancelled() {
        ArkErrorCode::RuntimeUnavailable
    } else {
        ArkErrorCode::StoreFailure
    };
    ArkError::new(
        code,
        format!("cannot {operation} for im channel {channel_id}: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use ark_common::{ArkError, ArkErrorCode, Result};
    use ark_contract::{ImBinding, ImChannelId};
    use ark_ports::{ImChannelRecord, ImInboxEntry, ImInboxId, ImStore, InboundMessage};

    use super::InboundSender;

    #[derive(Clone, Copy)]
    enum BatchBehavior {
        Fail,
        Block,
        Panic,
    }

    struct BlockingGate {
        entered: AtomicBool,
        release: Mutex<bool>,
        released: std::sync::Condvar,
    }

    impl BlockingGate {
        fn new() -> Self {
            Self {
                entered: AtomicBool::new(false),
                release: Mutex::new(false),
                released: std::sync::Condvar::new(),
            }
        }

        fn wait(&self) {
            self.entered.store(true, Ordering::Release);
            let mut release = self.release.lock().unwrap();
            while !*release {
                release = self.released.wait(release).unwrap();
            }
        }

        fn release(&self) {
            *self.release.lock().unwrap() = true;
            self.released.notify_all();
        }
    }

    struct BatchFailureStore {
        inbox: Mutex<Vec<ImInboxEntry>>,
        batch_writes: Mutex<usize>,
        behavior: BatchBehavior,
        gate: Option<Arc<BlockingGate>>,
    }

    impl Default for BatchFailureStore {
        fn default() -> Self {
            Self {
                inbox: Mutex::new(Vec::new()),
                batch_writes: Mutex::new(0),
                behavior: BatchBehavior::Fail,
                gate: None,
            }
        }
    }

    impl BatchFailureStore {
        fn with_behavior(behavior: BatchBehavior, gate: Option<Arc<BlockingGate>>) -> Self {
            Self {
                behavior,
                gate,
                ..Self::default()
            }
        }
    }

    impl ImStore for BatchFailureStore {
        fn list_channels(&self) -> Result<Vec<ImChannelRecord>> {
            unimplemented!()
        }

        fn get_channel(&self, _id: &ImChannelId) -> Result<ImChannelRecord> {
            unimplemented!()
        }

        fn insert_channel(&self, _record: ImChannelRecord) -> Result<ImChannelRecord> {
            unimplemented!()
        }

        fn replace_channel(&self, _record: ImChannelRecord) -> Result<ImChannelRecord> {
            unimplemented!()
        }

        fn delete_channel(&self, _id: &ImChannelId) -> Result<()> {
            unimplemented!()
        }

        fn list_bindings(&self, _channel_id: Option<&ImChannelId>) -> Result<Vec<ImBinding>> {
            unimplemented!()
        }

        fn get_binding(
            &self,
            _channel_id: &ImChannelId,
            _chat_id: &str,
        ) -> Result<Option<ImBinding>> {
            unimplemented!()
        }

        fn set_binding(&self, _binding: ImBinding) -> Result<()> {
            unimplemented!()
        }

        fn delete_binding(&self, _channel_id: &ImChannelId, _chat_id: &str) -> Result<()> {
            unimplemented!()
        }

        fn persist_inbound(
            &self,
            _message: InboundMessage,
            _next_cursor: Option<String>,
        ) -> Result<ImInboxId> {
            unimplemented!()
        }

        fn persist_inbound_batch(
            &self,
            messages: Vec<InboundMessage>,
            next_cursor: Option<String>,
        ) -> Result<Vec<ImInboxId>> {
            *self.batch_writes.lock().unwrap() += 1;
            if matches!(self.behavior, BatchBehavior::Fail) {
                return Err(ArkError::new(
                    ArkErrorCode::StoreFailure,
                    "injected atomic batch write failure",
                ));
            }
            if matches!(self.behavior, BatchBehavior::Panic) {
                panic!("injected persistence panic");
            }
            if let Some(gate) = &self.gate {
                gate.wait();
            }
            let ids: Vec<_> = messages.iter().map(|_| ImInboxId::new()).collect();
            self.inbox
                .lock()
                .unwrap()
                .extend(
                    ids.iter()
                        .cloned()
                        .zip(messages)
                        .map(|(id, message)| ImInboxEntry {
                            id,
                            message,
                            next_cursor: next_cursor.clone(),
                            queued: false,
                            cursor_version: None,
                        }),
                );
            Ok(ids)
        }

        fn list_inbox(&self) -> Result<Vec<ImInboxEntry>> {
            Ok(self.inbox.lock().unwrap().clone())
        }

        fn advance_channel_cursor(
            &self,
            _channel_id: &ImChannelId,
            _next: Option<String>,
        ) -> Result<()> {
            unimplemented!()
        }

        fn queue_inbound(&self, _id: &ImInboxId, _next_cursor: Option<String>) -> Result<()> {
            unimplemented!()
        }

        fn ack_inbound(&self, _id: &ImInboxId) -> Result<()> {
            unimplemented!()
        }

        fn discard_inbound(&self, _id: &ImInboxId, _next_cursor: Option<String>) -> Result<()> {
            unimplemented!()
        }
    }

    fn message(message_id: &str) -> InboundMessage {
        InboundMessage {
            channel_id: ImChannelId(String::new()),
            chat_id: "chat".into(),
            sender_id: "sender".into(),
            sender_name: "Sender".into(),
            message_id: message_id.into(),
            text: "hello".into(),
            is_dm: true,
            is_mention: false,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn large_batch_persistence_does_not_block_the_async_executor() {
        let gate = Arc::new(BlockingGate::new());
        let store = Arc::new(BatchFailureStore::with_behavior(
            BatchBehavior::Block,
            Some(Arc::clone(&gate)),
        ));
        let (sender, mut receiver) = InboundSender::new(store.clone(), ImChannelId("im_1".into()));
        let watchdog_gate = Arc::clone(&gate);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            watchdog_gate.release();
        });
        let started = Instant::now();
        let batch = tokio::spawn({
            let sender = sender.clone();
            async move {
                sender
                    .send_batch(
                        (0..256).map(|index| message(&index.to_string())).collect(),
                        Some("256".into()),
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while !gate.entered.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("async executor reaches the blocking store");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the async executor must keep running while im.json is rewritten"
        );
        gate.release();
        for _ in 0..256 {
            let delivery = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                .await
                .expect("delivery receiver remains responsive")
                .expect("delivery");
            delivery.acknowledge();
        }
        tokio::time::timeout(Duration::from_secs(1), batch)
            .await
            .expect("batch completes")
            .unwrap()
            .expect("persistence succeeds");
        assert_eq!(store.list_inbox().unwrap().len(), 256);
    }

    #[tokio::test]
    async fn blocking_persistence_panic_maps_to_contextual_store_failure() {
        let store = Arc::new(BatchFailureStore::with_behavior(BatchBehavior::Panic, None));
        let (sender, _receiver) = InboundSender::new(store, ImChannelId("im_1".into()));

        let error = sender
            .send_batch(vec![message("1")], None)
            .await
            .unwrap_err();

        assert_eq!(error.code, ArkErrorCode::StoreFailure);
        assert!(error.message.contains("persist inbound batch"));
        assert!(error.message.contains("im_1"));
    }

    #[tokio::test]
    async fn failed_batch_persistence_does_not_leave_a_partial_inbox() {
        let store = Arc::new(BatchFailureStore::default());
        let (sender, mut receiver) = InboundSender::new(store.clone(), ImChannelId("im_1".into()));

        let result = sender
            .send_batch(vec![message("1"), message("2")], Some("3".into()))
            .await;

        assert!(result.is_err(), "the injected write failure must surface");
        assert_eq!(*store.batch_writes.lock().unwrap(), 1);
        assert!(
            store.list_inbox().unwrap().is_empty(),
            "a failed platform batch must not leave a partially persisted inbox"
        );
        assert!(
            receiver.try_recv().is_err(),
            "no partial delivery is offered"
        );
    }
}
