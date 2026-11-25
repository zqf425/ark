//! Telegram Bot API transport (requirements/im-channels.md, stage 2).
//!
//! Zero new dependencies: the injected [`reqwest::Client`] talks to
//! `api.telegram.org` directly. `getUpdates` long-polls with a 25s server
//! window; every request carries its own client-side deadline (poll window
//! plus margin) because the injected client is built with
//! `client_timeout = None` for streaming callers.
//!
//! v1 is private-chat only: group/supergroup/channel updates are reported with
//! `is_dm = false` and the orchestration drops them (requirements «平台适配器
//! 要点»), so this adapter never has to resolve group permissions.

use std::sync::Mutex;
use std::time::Duration;

use ark_common::{ArkError, Result};
use ark_contract::ImChannelKind;
use ark_ports::CancelToken;
use async_trait::async_trait;
use serde_json::Value;

use crate::domain::{chunk_text, truncate_inbound, InboundMessage};
use crate::InboundSender;

/// Server-side long-poll window; the client deadline adds margin for TLS and
/// body drain.
const POLL_WINDOW_SECS: u64 = 25;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(POLL_WINDOW_SECS + 10);
/// Backoff floor/ceiling with jitter for the reconnect curve.
const BACKOFF_MIN: Duration = Duration::from_millis(1500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

pub struct TelegramTransport {
    client: reqwest::Client,
    /// `https://api.telegram.org/bot{token}` — built once, no token logging.
    base: String,
    /// Filled from `getMe` when known; lets `is_self` work without a second
    /// lookup even before the first poll returns.
    bot_user_id: Mutex<Option<String>>,
    /// Next `getUpdates` offset (decimal string); `None` = server-side default.
    cursor: Mutex<Option<String>>,
}

impl TelegramTransport {
    pub fn new(
        client: reqwest::Client,
        bot_token: String,
        bot_username: Option<String>,
        cursor: Option<String>,
    ) -> Self {
        let _ = bot_username; // recorded on the channel, not needed per request
        Self {
            client,
            base: format!("https://api.telegram.org/bot{}", bot_token.trim()),
            bot_user_id: Mutex::new(None),
            cursor: Mutex::new(cursor),
        }
    }

    fn set_cursor(&self, offset: i64) {
        *self
            .cursor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(offset.to_string());
    }

    async fn call(&self, method: &str, body: Value) -> Result<Value> {
        let url = format!("{}/{}", self.base, method);
        let response = self
            .client
            .post(url)
            .timeout(REQUEST_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                ArkError::new(
                    ark_common::ArkErrorCode::ToolExecutionFailed,
                    format!("telegram {method} failed: {err}"),
                )
            })?;
        let payload: Value = response.json().await.map_err(|err| {
            ArkError::new(
                ark_common::ArkErrorCode::ToolExecutionFailed,
                format!("telegram {method} returned non-JSON: {err}"),
            )
        })?;
        match payload.get("ok").and_then(Value::as_bool) {
            Some(true) => Ok(payload),
            _ => {
                let description = payload
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                Err(ArkError::new(
                    ark_common::ArkErrorCode::ToolExecutionFailed,
                    format!("telegram {method} error: {description}"),
                ))
            }
        }
    }

    async fn send_plain(&self, chat_id: &str, text: &str) -> Result<()> {
        self.call(
            "sendMessage",
            serde_json::json!({ "chat_id": chat_id, "text": text }),
        )
        .await
        .map(|_| ())
    }
}

/// Escape the five HTML-significant characters so replies cannot inject markup.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[async_trait]
impl crate::ImTransport for TelegramTransport {
    fn kind(&self) -> ImChannelKind {
        ImChannelKind::Telegram
    }

    async fn run(&self, inbound: InboundSender, shutdown: CancelToken) -> Result<()> {
        // One identity lookup up front: it validates the token (fail fast
        // instead of a poll loop that 401s forever) and enables self-filtering.
        let me = self.call("getMe", serde_json::json!({})).await?;
        if let Some(id) = me
            .pointer("/result/id")
            .and_then(Value::as_i64)
            .map(|id| id.to_string())
        {
            *self
                .bot_user_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id);
        }

        let mut backoff = BACKOFF_MIN;
        loop {
            if shutdown.is_cancelled() {
                return Ok(());
            }
            let mut body = serde_json::json!({
                "timeout": POLL_WINDOW_SECS,
                "allowed_updates": ["message"],
            });
            if let Some(offset) = self
                .cursor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
            {
                body["offset"] = Value::from(offset);
            }
            let result = self.call("getUpdates", body).await;
            let updates = match result {
                Ok(payload) => {
                    backoff = BACKOFF_MIN;
                    payload
                        .get("result")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                }
                Err(err) => {
                    // A transient network error must not kill the connection:
                    // back off and retry until the shutdown token fires.
                    tracing::warn!(error = %err.message, "telegram getUpdates failed; backing off");
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = shutdown.cancelled() => return Ok(()),
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    continue;
                }
            };
            let (messages, next_cursor) = normalize_updates(&updates);
            if let Some(next) = next_cursor {
                if messages.is_empty() {
                    inbound.advance(Some(next.to_string())).await?;
                } else {
                    inbound.send_batch(messages, Some(next.to_string())).await?;
                }
                // Only durable admission of the entire batch makes this local
                // resume cursor safe to expose to the next poll.
                self.set_cursor(next);
            }
        }
    }

    async fn send_text(&self, chat_id: &str, text: &str) -> Result<()> {
        // HTML mode renders the reply with basic fidelity; a platform rejection
        // (bad entity, oversized after escaping) falls back to plain text.
        for chunk in chunk_text(self.kind(), text) {
            let html = escape_html(&chunk);
            let sent = self
                .call(
                    "sendMessage",
                    serde_json::json!({
                        "chat_id": chat_id,
                        "text": html,
                        "parse_mode": "HTML",
                    }),
                )
                .await;
            if sent.is_err() {
                self.send_plain(chat_id, &chunk).await?;
            }
        }
        Ok(())
    }

    async fn verify(&self) -> Result<String> {
        let me = self.call("getMe", serde_json::json!({})).await?;
        let username = me
            .pointer("/result/username")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        Ok(format!("@{username}"))
    }

    fn is_self(&self, sender_id: &str) -> bool {
        self.bot_user_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_deref()
            == Some(sender_id)
    }
}

/// Normalize one poll batch and retain its final offset, including updates
/// whose payload is filtered out (media, unsupported update types).
fn normalize_updates(updates: &[Value]) -> (Vec<InboundMessage>, Option<i64>) {
    let mut messages = Vec::new();
    let mut next_cursor = None;
    for update in updates {
        if let Some(offset) = update.get("update_id").and_then(Value::as_i64) {
            next_cursor = Some(offset + 1);
        }
        if let Some(message) = update.get("message").filter(|m| m.is_object()) {
            if let Some(inbound) = normalize_message(message) {
                messages.push(inbound);
            }
        }
    }
    (messages, next_cursor)
}

/// Fold one Telegram message object into the normalized shape, or `None` for
/// everything the v1 scope cannot act on (media-only, no text).
fn normalize_message(message: &Value) -> Option<InboundMessage> {
    let chat = message.get("chat")?;
    let chat_type = chat.get("type").and_then(Value::as_str)?;
    let chat_id = match chat.get("id") {
        Some(Value::Number(id)) => id.to_string(),
        _ => return None,
    };
    let text = message.get("text").and_then(Value::as_str)?;
    let sender = message.get("from");
    let sender_id = sender
        .and_then(|from| from.get("id"))
        .and_then(|value| value.as_i64())
        .map(|id| id.to_string())
        .unwrap_or_default();
    let sender_name = sender
        .map(|from| {
            from.get("first_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .unwrap_or_default();
    Some(InboundMessage {
        channel_id: ark_contract::ImChannelId(String::new()),
        chat_id,
        sender_id,
        sender_name: sender_name.to_string(),
        message_id: message
            .get("message_id")
            .and_then(Value::as_i64)
            .map(|id| id.to_string())
            .unwrap_or_default(),
        text: truncate_inbound(text),
        is_dm: chat_type == "private",
        is_mention: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::KindLimits;

    #[test]
    fn html_escaping_neutralizes_markup() {
        assert_eq!(escape_html("<b>&\"x\">"), "&lt;b&gt;&amp;&quot;x&quot;&gt;");
    }

    #[test]
    fn message_normalization_keeps_private_chat_text_and_drops_the_rest() {
        let message: Value = serde_json::from_str(
            r#"{
                "message_id": 7,
                "from": { "id": 42, "first_name": "Ann" },
                "chat": { "id": 42, "type": "private" },
                "text": "hello"
            }"#,
        )
        .unwrap();
        let normalized = normalize_message(&message).unwrap();
        assert_eq!(normalized.chat_id, "42");
        assert!(normalized.is_dm);
        assert_eq!(normalized.sender_name, "Ann");
        assert_eq!(normalized.text, "hello");

        let group: Value = serde_json::from_str(
            r#"{ "message_id": 8, "from": { "id": 1 }, "chat": { "id": -9, "type": "group" }, "text": "hi" }"#,
        )
        .unwrap();
        let normalized = normalize_message(&group).unwrap();
        assert!(!normalized.is_dm);

        let sticker: Value = serde_json::from_str(
            r#"{ "message_id": 9, "chat": { "id": 1, "type": "private" }, "sticker": {} }"#,
        )
        .unwrap();
        assert!(normalize_message(&sticker).is_none());
    }

    #[test]
    fn telegram_limit_comes_from_the_kind_table() {
        assert_eq!(ImChannelKind::Telegram.max_message_chars(), 4096);
    }
}
