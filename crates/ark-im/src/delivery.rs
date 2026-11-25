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
        let ids = self
            .store
            .persist_inbound_batch(messages.clone(), next_cursor.clone())?;
        if ids.len() != messages.len() {
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
        self.store
            .advance_channel_cursor(&self.channel_id, next_cursor)
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use ark_common::{ArkError, ArkErrorCode, Result};
    use ark_contract::{ImBinding, ImChannelId};
    use ark_ports::{ImChannelRecord, ImInboxEntry, ImInboxId, ImStore, InboundMessage};

    use super::InboundSender;

    #[derive(Default)]
    struct BatchFailureStore {
        inbox: Mutex<Vec<ImInboxEntry>>,
        batch_writes: Mutex<usize>,
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
            _messages: Vec<InboundMessage>,
            _next_cursor: Option<String>,
        ) -> Result<Vec<ImInboxId>> {
            *self.batch_writes.lock().unwrap() += 1;
            Err(ArkError::new(
                ArkErrorCode::StoreFailure,
                "injected atomic batch write failure",
            ))
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
