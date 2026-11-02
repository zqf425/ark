//! Wire types for IM channel integration (docs/requirements/im-channels.md).
//!
//! These shapes are the single source for the persisted `im.json` channel
//! entries, the HTTP payloads and the generated TypeScript. Policy decisions
//! (whitelist semantics, mention gating) and credential validation live in
//! `ark-im`; this module deliberately carries only shapes and the constants
//! both the UI and the runtime need to agree on.
//!
//! Write/read models are separate (design §6.1): `ImChannel` is the stored and
//! server-internal shape, `CreateImChannelRequest`/`UpdateImChannelRequest`
//! are the payloads (mirroring the schedule trio), and `ImChannelView` is the
//! redacted read model. Secrets never round-trip: the write model accepts them,
//! the read model only reports whether they are set.

use ark_common::ThreadId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum number of channels the store accepts.
///
/// Exported so the settings UI can disable "create" at the same boundary the
/// runtime enforces, instead of hard-coding a second copy of the number.
pub const MAX_IM_CHANNELS: usize = 20;

/// Maximum UTF-8 byte length of a channel title.
pub const IM_TITLE_MAX_BYTES: usize = 200;

/// Autonomy applied to IM-triggered turns when the channel names none.
///
/// IM channels deliberately default to `unattended` (like scheduled tasks):
/// nobody is sitting at the desktop to answer an approval gate. Combined with
/// the default-closed whitelist this keeps "configured credentials" from
/// meaning "any stranger can run tools on this machine" (see
/// `ImChannelPolicy::allowed_chat_ids`).
pub const DEFAULT_IM_AUTONOMY: &str = "unattended";

/// Newtype channel id (design §6.3): `im_<uuid>`, assigned by the store when a
/// create request omits it. A distinct type stops chat ids, thread ids and
/// channel ids from being swapped silently.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[serde(transparent)]
pub struct ImChannelId(pub String);

impl ImChannelId {
    pub fn new() -> Self {
        Self(format!("im_{}", uuid::Uuid::new_v4()))
    }
}

impl Default for ImChannelId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ImChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One connected IM platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum ImChannelKind {
    #[default]
    Telegram,
    Feishu,
    Weixin,
}

/// Feishu deployment the credentials belong to. The two domains use different
/// API hosts and different account systems, so one app id cannot serve both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum FeishuDomain {
    #[default]
    Feishu,
    Lark,
}

/// Channel credentials, discriminated by `kind`.
///
/// This is the *write* shape: it holds plaintext secrets and only ever appears
/// in request bodies and the persisted document, never in a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImCredentials {
    Telegram {
        bot_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_username: Option<String>,
    },
    Feishu {
        app_id: String,
        app_secret: String,
        #[serde(default)]
        domain: FeishuDomain,
    },
    Weixin {
        bot_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_url: Option<String>,
        /// The account the QR login bound as owner. Required: a WeChat bot
        /// without an owner would execute unattended turns for whoever sends
        /// the first message.
        owner_user_id: String,
    },
}

impl ImCredentials {
    /// `true` when every secret field this variant requires is non-empty.
    ///
    /// Non-secret optional fields (`bot_username`, `base_url`) do not gate
    /// validity: the adapters can work without them.
    pub fn secrets_present(&self) -> bool {
        match self {
            ImCredentials::Telegram { bot_token, .. } => !bot_token.trim().is_empty(),
            ImCredentials::Feishu {
                app_id, app_secret, ..
            } => !app_id.trim().is_empty() && !app_secret.trim().is_empty(),
            ImCredentials::Weixin {
                bot_token,
                owner_user_id,
                ..
            } => !bot_token.trim().is_empty() && !owner_user_id.trim().is_empty(),
        }
    }

    /// Redact into the read model: non-secret fields stay, secrets become
    /// `has_*` booleans.
    pub fn to_view(&self) -> ImCredentialsView {
        match self {
            ImCredentials::Telegram {
                bot_username,
                bot_token,
            } => ImCredentialsView::Telegram {
                bot_username: bot_username.clone(),
                has_bot_token: !bot_token.trim().is_empty(),
            },
            ImCredentials::Feishu {
                app_id,
                app_secret,
                domain,
            } => ImCredentialsView::Feishu {
                app_id: app_id.clone(),
                domain: *domain,
                has_app_secret: !app_secret.trim().is_empty(),
            },
            ImCredentials::Weixin {
                base_url,
                owner_user_id,
                bot_token,
            } => ImCredentialsView::Weixin {
                base_url: base_url.clone(),
                owner_user_id: owner_user_id.clone(),
                has_bot_token: !bot_token.trim().is_empty(),
            },
        }
    }
}

/// Redacted credential read model (`ProviderView` precedent): the settings UI
/// can render *what is configured* without ever receiving the secret back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImCredentialsView {
    Telegram {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_username: Option<String>,
        has_bot_token: bool,
    },
    Feishu {
        app_id: String,
        domain: FeishuDomain,
        has_app_secret: bool,
    },
    Weixin {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_url: Option<String>,
        owner_user_id: String,
        has_bot_token: bool,
    },
}

/// Who may talk to this channel.
///
/// `allowed_chat_ids` defaults to *closed*: an empty list answers a new chat
/// with a hint naming its chat id (first-run onboarding), and a non-empty list
/// silently drops chats that are not on it. Both branches are deliberate —
/// the channel triggers unattended tool execution, so "anyone who finds the
/// bot" must never be the effective policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ImChannelPolicy {
    #[serde(default)]
    pub allowed_chat_ids: Vec<String>,
    /// Group messages must @-mention the bot. Serde default keeps older
    /// documents readable; `ark-im`'s normalization re-applies it for creates.
    #[serde(default = "default_true")]
    pub require_mention: bool,
}

fn default_true() -> bool {
    true
}

/// One configured channel: credentials, policy and the session overrides every
/// IM-triggered turn inherits.
///
/// Stored shape (`im.json`) and server-internal shape; it never appears in a
/// request body (`CreateImChannelRequest`/`UpdateImChannelRequest` are the
/// payloads) and never in a response (`ImChannelView` redacts credentials).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ImChannel {
    #[serde(default)]
    pub id: ImChannelId,
    #[serde(default)]
    pub kind: ImChannelKind,
    #[serde(default)]
    pub title: String,
    /// Missing flag reads as disabled: a damaged or older document must not
    /// open a long connection nobody asked for.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub credentials: Option<ImCredentials>,
    #[serde(default)]
    pub policy: ImChannelPolicy,
    /// Empty/None means the runtime's default workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Empty means the runtime's default provider/model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// `strict` | `workspace_write` | `unattended`; empty falls back to
    /// `DEFAULT_IM_AUTONOMY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<String>,
    #[serde(default)]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: DateTime<Utc>,
}

/// `POST /v1/im/channels`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct CreateImChannelRequest {
    #[serde(default)]
    pub kind: ImChannelKind,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub credentials: Option<ImCredentials>,
    #[serde(default)]
    pub policy: Option<ImChannelPolicy>,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub autonomy: Option<String>,
}

/// `PATCH /v1/im/channels/{id}`.
///
/// Every present field replaces the stored value; absent fields keep it.
/// Clearing an optional text field (`workspace`, `model`, `provider_id`,
/// `autonomy`) is expressed by an empty string, folded back to "unset" by
/// normalization. A secret field that is empty or absent keeps the stored
/// secret (same rule as provider `api_key`), so an edit form that does not
/// re-display secrets cannot wipe them by saving.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct UpdateImChannelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<ImCredentials>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<ImChannelPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<String>,
}

/// Redacted read model: the channel as `GET`/`PATCH` responses return it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ImChannelView {
    pub id: ImChannelId,
    pub kind: ImChannelKind,
    pub title: String,
    pub enabled: bool,
    pub credentials: Option<ImCredentialsView>,
    pub policy: ImChannelPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ImChannel {
    pub fn to_view(&self) -> ImChannelView {
        ImChannelView {
            id: self.id.clone(),
            kind: self.kind,
            title: self.title.clone(),
            enabled: self.enabled,
            credentials: self.credentials.as_ref().map(ImCredentials::to_view),
            policy: self.policy.clone(),
            workspace: self.workspace.clone(),
            model: self.model.clone(),
            provider_id: self.provider_id.clone(),
            autonomy: self.autonomy.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Live connection state the orchestration maintains; never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum ImChannelState {
    #[default]
    Disabled,
    Connecting,
    Online,
    /// Degraded Feishu mode: outbound REST still works, inbound long
    /// connection does not (requirements/im-channels.md, stage 3).
    SendOnly,
    Error,
}

/// One `GET /v1/im/channels` entry: redacted channel plus live state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ImChannelListItem {
    pub channel: ImChannelView,
    pub runtime: ImChannelRuntime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ImChannelRuntime {
    #[serde(default)]
    pub state: ImChannelState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_inbound_at: Option<DateTime<Utc>>,
}

impl Default for ImChannelRuntime {
    fn default() -> Self {
        Self {
            state: ImChannelState::Disabled,
            last_error: None,
            last_inbound_at: None,
        }
    }
}

/// `GET /v1/im/channels` — every channel with its live state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ImChannelListResponse {
    #[serde(default)]
    pub channels: Vec<ImChannelListItem>,
}

/// `POST /v1/im/channels/{id}/verify` — the platform identity the credentials
/// resolve to (`@bot_username`, an app name, a WeChat owner id).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ImVerifyResponse {
    #[serde(default)]
    pub identity: String,
}

/// Durable mapping from one platform chat to one ark thread.
///
/// Written when the first inbound message creates the thread, removed by
/// `/new`. `(channel_id, chat_id)` is the conversation key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ImBinding {
    pub channel_id: ImChannelId,
    pub chat_id: String,
    pub thread_id: ThreadId,
    pub updated_at: DateTime<Utc>,
}

/// `GET /v1/im/bindings` — the read-only mapping table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ImBindingListResponse {
    #[serde(default)]
    pub bindings: Vec<ImBinding>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_and_credentials_use_snake_case_discriminators() {
        assert_eq!(
            serde_json::to_string(&ImChannelKind::Telegram).unwrap(),
            "\"telegram\""
        );
        let json = r#"{"kind":"feishu","app_id":"a","app_secret":"s","domain":"lark"}"#;
        let creds: ImCredentials = serde_json::from_str(json).unwrap();
        assert_eq!(
            creds,
            ImCredentials::Feishu {
                app_id: "a".into(),
                app_secret: "s".into(),
                domain: FeishuDomain::Lark,
            }
        );
    }

    #[test]
    fn view_redacts_secrets_and_keeps_non_secret_fields() {
        let creds = ImCredentials::Telegram {
            bot_token: "tok".into(),
            bot_username: Some("ark_bot".into()),
        };
        let ImCredentialsView::Telegram {
            bot_username,
            has_bot_token,
        } = creds.to_view()
        else {
            panic!("variant must survive redaction");
        };
        assert_eq!(bot_username.as_deref(), Some("ark_bot"));
        assert!(has_bot_token);
    }

    #[test]
    fn policy_require_mention_defaults_to_true() {
        let policy: ImChannelPolicy = serde_json::from_str("{}").unwrap();
        assert!(policy.require_mention);
    }

    #[test]
    fn channel_id_serializes_as_a_plain_string() {
        let id = ImChannelId("im_1".into());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"im_1\"");
    }
}
