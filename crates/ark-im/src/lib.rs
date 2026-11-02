//! ark-im: IM channel platform adapters (docs/requirements/im-channels.md).
//!
//! The crate owns the platform edge and nothing else: it turns platform byte
//! streams into normalized [`domain::InboundMessage`]s and outbound text into
//! platform messages. Routing, bindings, turn driving and reply orchestration
//! live in `ark-server`'s `service/im.rs`; persistence lives behind
//! [`ark_ports::ImStore`]. Nothing here knows about threads, turns, config
//! snapshots or the file system.
//!
//! Outbound HTTP governance: adapters never build their own
//! [`reqwest::Client`] — the composition root constructs it via
//! `ark_http::transport_client` (the same `HttpPolicy`-derived proxy/TLS
//! settings model traffic uses) and injects it into every constructor. Long
//! polling needs `client_timeout = None` at that call site; the per-request
//! deadline is the adapter's own polling window.

#![deny(clippy::all)]

pub mod domain;
pub mod feishu;
pub mod telegram;

pub use domain::{
    apply_update, build_transport, channel_autonomy, chunk_text, ensure_channel_capacity,
    parse_command, prepare_create, truncate_inbound, validate_credentials, Command, DedupSet,
    InboundMessage, KindLimits, DEDUP_CAPACITY,
};
pub use feishu::FeishuTransport;
pub use telegram::TelegramTransport;

use ark_contract::ImChannelKind;
use ark_ports::CancelToken;
use async_trait::async_trait;
use tokio::sync::mpsc;

/// One platform connection: normalize inbound, send outbound.
///
/// One instance per channel, built by the orchestration with an injected
/// [`reqwest::Client`] and already-resolved credentials. `run` consumes a
/// shutdown token and either runs until it is cancelled or returns `Err` on a
/// fatal setup error (the orchestration folds that into
/// `ImChannelRuntime::error` and decides about retries).
#[async_trait]
pub trait ImTransport: Send + Sync {
    fn kind(&self) -> ImChannelKind;

    /// Drive the connection until [`CancelToken::cancelled`] resolves.
    ///
    /// Normalized messages go to `inbound`. The adapter reports only platform
    /// facts (`is_dm`, `is_mention`); policy filtering (whitelist, mention
    /// gating, self-messages) belongs to the orchestration.
    async fn run(
        &self,
        inbound: mpsc::Sender<InboundMessage>,
        shutdown: CancelToken,
    ) -> ark_common::Result<()>;

    /// Send text to one chat, chunking to the platform's per-message limit
    /// (the orchestration hands over the full reply).
    async fn send_text(&self, chat_id: &str, text: &str) -> ark_common::Result<()>;

    /// Validate the credentials against the platform, returning a short
    /// identity description (`@bot_username`, an app name, an owner id).
    async fn verify(&self) -> ark_common::Result<String>;

    /// The latest inbound cursor to persist, if this adapter tracks one.
    ///
    /// The orchestration reads this on its sync sweep and persists it through
    /// [`ark_ports::ImStore`] (throttled), so a restart resumes where the
    /// platform left off instead of replaying a whole window.
    fn cursor(&self) -> Option<String> {
        None
    }

    /// Whether `sender_id` is the bot's own account (its own messages must not
    /// trigger a turn). Platforms differ in whether they echo self-messages
    /// back; Telegram does not, Feishu and WeChat do.
    fn is_self(&self, sender_id: &str) -> bool {
        let _ = sender_id;
        false
    }
}

/// Interactive credential acquisition (WeChat QR login, stage 4).
///
/// Held by a dedicated object assembled in the composition root — it must not
/// depend on already-parsed credentials, because the credentials are the
/// *product* of the login. `LoginSession` is short-lived in-process state
/// (expires in minutes, at most one concurrent session); on success the
/// orchestration stores the channel and reconciles.
pub trait ImLoginFlow: Send + Sync {
    fn start(&self) -> ark_common::Result<LoginSession>;
    async fn poll(&self, session: &LoginSession) -> ark_common::Result<LoginPoll>;
}

/// A pending interactive login: what the UI renders as a QR code.
pub struct LoginSession {
    pub id: String,
    /// Platform-provided QR payload (usually a URL to encode).
    pub qr_content: String,
}

/// One poll of a pending login.
pub enum LoginPoll {
    /// Waiting for the user to scan.
    Pending,
    /// Scanned, waiting for confirmation on the phone.
    Scanned,
    /// Confirmed: credentials ready to store.
    Confirmed {
        credentials: ark_contract::ImCredentials,
        title: String,
    },
    /// The QR code expired before confirmation.
    Expired,
    /// The platform rejected the login.
    Failed(String),
}
