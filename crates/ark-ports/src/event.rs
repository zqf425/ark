//! Event fan-out port.
//!
//! The writer owns sequence numbers (design §7.9); subscribers only receive.
//! `Subscription` is where the "snapshot then live" atomicity promise lands, so
//! it hands back both the backlog and the live stream together.

use ark_common::{Result, ThreadId};
use ark_contract::RuntimeEvent;
use tokio::sync::mpsc;

/// A live subscription to one thread's event stream.
///
/// `backlog` is `(since_seq, watermark]` already durable on disk, `watermark`
/// is the last backlogged `seq`, and `live` only ever yields `seq > watermark`.
/// Concatenating the two is therefore already ordered and gap-free.
pub struct Subscription {
    pub backlog: Vec<RuntimeEvent>,
    pub watermark: u64,
    pub live: mpsc::Receiver<RuntimeEvent>,
}

/// Publish side of the event bus.
#[async_trait::async_trait]
pub trait EventSink: Send + Sync {
    /// Append one event, assigning the next `seq` for the thread.
    async fn publish(&self, thread: ThreadId, event: RuntimeEvent) -> Result<RuntimeEvent>;

    /// Atomically determine the watermark, read the backlog and register the
    /// live channel. Implementations must not interleave a concurrent append.
    async fn subscribe(&self, thread: ThreadId, since_seq: u64) -> Result<Subscription>;
}
