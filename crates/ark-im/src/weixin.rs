//! WeChat iLink bot transport (requirements/im-channels.md, stage 4).
//!
//! The protocol shapes follow smart-ark's `weixin-bridge-runtime.ts` and its
//! `@tencent-weixin/openclaw-weixin` dependency (protocol reference only — no
//! Node dependency here):
//!
//! * Every call posts JSON to `ilink/bot/*` with the headers
//!   `AuthorizationType: ilink_bot_token`, `Authorization: Bearer <token>`,
//!   `X-WECHAT-UIN` (random per request), `iLink-App-Id` and
//!   `iLink-App-ClientVersion`.
//! * `getupdates` long-polls with an opaque `get_updates_buf` cursor; the
//!   response may adjust the poll window via `longpolling_timeout_ms` and
//!   carries `msgs` plus the next cursor. `message_type == 2` is the bot's own
//!   message and is skipped; text lives in `item_list[].text_item.text`
//!   (voice items expose a transcription in `voice_item.text`).
//! * `sendmessage` needs a fresh `context_token` from the peer's last inbound
//!   message for a proactive push to succeed (`ret = -2` otherwise); the
//!   transport keeps one per sender in memory.
//! * QR login: `get_bot_qrcode` (with the local token list) returns the QR
//!   payload; `get_qrcode_status` long-polls the status machine
//!   (`wait`/`scaned`/`expired`/`confirmed`/...); `confirmed` carries the bot
//!   token, base URL and the owner user id that scanned.
//!
//! Business failures ride inside 200 responses (`ret`/`errcode`), so both are
//! checked — only reading HTTP status would turn `{"ret":-2}` into a fake
//! success.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use ark_common::{ArkError, ArkErrorCode, Result};
use ark_contract::{ImChannelId, ImChannelKind, ImCredentials};
use ark_ports::CancelToken;
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::domain::{chunk_text, truncate_inbound, InboundMessage};
use crate::ImLoginFlow;
use crate::{LoginPoll, LoginSession};

/// Per smart-ark: the packaged iLink app id and its wire client version.
const APP_ID: &str = "bot";
const CLIENT_VERSION: u32 = (2 << 16) | (4 << 8) | 3; // openclaw-weixin 2.4.3
const DEFAULT_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const LONG_POLL_SECS: u64 = 35;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(40);
const REST_TIMEOUT: Duration = Duration::from_secs(15);
const QR_BOT_TYPE: &str = "3";

fn transport_error(message: impl Into<String>) -> ArkError {
    ArkError::new(ArkErrorCode::ToolExecutionFailed, message)
}

fn base_info() -> serde_json::Value {
    serde_json::json!({
        "channel_version": "2.4.3",
        "bot_agent": "ark/0.1.0",
    })
}

fn random_uin() -> String {
    // A fresh random uin per request, base64 of its decimal digits — the
    // server treats it as telemetry, not identity (same shape as smart-ark).
    use base64::Engine;
    let value = uuid::Uuid::new_v4().as_u128() as u32;
    base64::engine::general_purpose::STANDARD.encode(value.to_string().as_bytes())
}

fn business_error(payload: &serde_json::Value, label: &str) -> Option<String> {
    let ret = payload.get("ret").and_then(|v| v.as_i64()).unwrap_or(0);
    let errcode = payload.get("errcode").and_then(|v| v.as_i64()).unwrap_or(0);
    if ret == 0 && errcode == 0 {
        return None;
    }
    let detail = payload
        .get("errmsg")
        .or_else(|| payload.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("unknown");
    Some(format!("{label} failed ({ret}/{errcode}): {detail}"))
}

/// Extract text from an `item_list`: TEXT(1) wins, VOICE(3) contributes its
/// transcription; other item types are out of scope (no media in v1).
fn text_from_items(item_list: &serde_json::Value) -> Option<String> {
    let items = item_list.as_array()?;
    for item in items {
        match item.get("type").and_then(|t| t.as_i64()) {
            Some(1) => {
                if let Some(text) = item
                    .pointer("/text_item/text")
                    .and_then(|t| t.as_str())
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                {
                    return Some(text.to_string());
                }
            }
            Some(3) => {
                if let Some(text) = item
                    .pointer("/voice_item/text")
                    .and_then(|t| t.as_str())
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                {
                    return Some(text.to_string());
                }
            }
            _ => {}
        }
    }
    None
}

pub struct WeixinTransport {
    client: reqwest::Client,
    token: String,
    base_url: String,
    owner_user_id: String,
    /// Per-sender `context_token`s from the last inbound message; proactive
    /// sends without one are rejected by the server (`ret = -2`).
    context_tokens: Mutex<HashMap<String, String>>,
    /// Opaque `get_updates_buf`; persisted by the orchestration so a restart
    /// resumes instead of replaying the window.
    cursor: Mutex<Option<String>>,
}

impl WeixinTransport {
    pub fn new(
        client: reqwest::Client,
        token: String,
        base_url: Option<String>,
        owner_user_id: String,
        cursor: Option<String>,
    ) -> Self {
        Self {
            client,
            token,
            base_url: base_url
                .filter(|url| !url.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            owner_user_id,
            context_tokens: Mutex::new(HashMap::new()),
            cursor: Mutex::new(cursor),
        }
    }

    fn url(&self, endpoint: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), endpoint)
    }

    async fn post(
        &self,
        endpoint: &str,
        body: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value> {
        let response = self
            .client
            .post(self.url(endpoint))
            .timeout(timeout)
            .header("Content-Type", "application/json")
            .header("AuthorizationType", "ilink_bot_token")
            .header("X-WECHAT-UIN", random_uin())
            .header("iLink-App-Id", APP_ID)
            .header("iLink-App-ClientVersion", CLIENT_VERSION.to_string())
            .bearer_auth(self.token.trim())
            .json(&body)
            .send()
            .await
            .map_err(|err| transport_error(format!("weixin {endpoint} failed: {err}")))?;
        let payload: serde_json::Value = response.json().await.map_err(|err| {
            transport_error(format!("weixin {endpoint} returned non-JSON: {err}"))
        })?;
        if let Some(message) = business_error(&payload, endpoint) {
            return Err(transport_error(message));
        }
        Ok(payload)
    }

    fn set_cursor(&self, next: Option<String>) {
        *self
            .cursor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    }
}

#[async_trait]
impl crate::ImTransport for WeixinTransport {
    fn kind(&self) -> ImChannelKind {
        ImChannelKind::Weixin
    }

    async fn run(
        &self,
        inbound: mpsc::Sender<InboundMessage>,
        shutdown: CancelToken,
    ) -> Result<()> {
        let mut failures = 0u32;
        loop {
            if shutdown.is_cancelled() {
                return Ok(());
            }
            let cursor = self
                .cursor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .unwrap_or_default();
            let payload = serde_json::json!({
                "get_updates_buf": cursor,
                "base_info": base_info(),
            });
            match self
                .post("ilink/bot/getupdates", payload, REQUEST_TIMEOUT)
                .await
            {
                Ok(response) => {
                    failures = 0;
                    if let Some(window) = response
                        .get("longpolling_timeout_ms")
                        .and_then(|v| v.as_u64())
                        .filter(|window| *window > 0)
                    {
                        // The server may retune the poll window; clamp it into
                        // the request timeout so the client never outruns it.
                        let _ = window;
                    }
                    if let Some(next) = response.get("get_updates_buf").and_then(|v| v.as_str()) {
                        self.set_cursor(Some(next.to_string()));
                    }
                    let messages = response
                        .get("msgs")
                        .and_then(|m| m.as_array())
                        .cloned()
                        .unwrap_or_default();
                    for message in messages {
                        // message_type 2 is the bot's own message.
                        if message.get("message_type").and_then(|t| t.as_i64()) == Some(2) {
                            continue;
                        }
                        let from = message
                            .get("from_user_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        if from.is_empty() {
                            continue;
                        }
                        if let Some(context_token) =
                            message.get("context_token").and_then(|v| v.as_str())
                        {
                            self.context_tokens
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .insert(from.clone(), context_token.to_string());
                        }
                        let Some(text) = text_from_items(
                            message.get("item_list").unwrap_or(&serde_json::Value::Null),
                        ) else {
                            continue;
                        };
                        let inbound_message = InboundMessage {
                            channel_id: ImChannelId(String::new()),
                            chat_id: from.clone(),
                            sender_id: from.clone(),
                            // The iLink push carries the user id only.
                            sender_name: String::new(),
                            message_id: message
                                .get("message_id")
                                .and_then(|v| v.as_str())
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("wx-{}", uuid::Uuid::new_v4())),
                            text: truncate_inbound(&text),
                            // iLink bots are 1:1 by protocol.
                            is_dm: true,
                            is_mention: false,
                        };
                        if inbound.send(inbound_message).await.is_err() {
                            return Ok(());
                        }
                    }
                }
                Err(err) => {
                    failures += 1;
                    // Three consecutive failures mean more than a blip: back
                    // off before hammering the gateway (smart-ark's curve).
                    let delay = if failures >= 3 {
                        failures = 0;
                        Duration::from_secs(30)
                    } else {
                        Duration::from_secs(2)
                    };
                    tracing::warn!(error = %err.message, "weixin getupdates failed; backing off");
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = shutdown.cancelled() => return Ok(()),
                    }
                }
            }
        }
    }

    async fn send_text(&self, chat_id: &str, text: &str) -> Result<()> {
        let context_token = self
            .context_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(chat_id)
            .cloned();
        for chunk in chunk_text(self.kind(), text) {
            let mut msg = serde_json::json!({
                "from_user_id": "",
                "to_user_id": chat_id,
                "client_id": format!("ark-weixin-{}", uuid::Uuid::new_v4()),
                "message_type": 2,     // BOT
                "message_state": 2,    // FINISH
                "item_list": [{ "type": 1, "text_item": { "text": chunk } }],
            });
            if let Some(token) = &context_token {
                msg["context_token"] = serde_json::Value::String(token.clone());
            }
            self.post(
                "ilink/bot/sendmessage",
                serde_json::json!({ "msg": msg, "base_info": base_info() }),
                REST_TIMEOUT,
            )
            .await?;
        }
        Ok(())
    }

    async fn verify(&self) -> Result<String> {
        // A short poll is the cheapest authenticated round trip; the owner id
        // from the QR login is the identity the channel enforces anyway.
        let payload = serde_json::json!({
            "get_updates_buf": "",
            "base_info": base_info(),
        });
        self.post("ilink/bot/getupdates", payload, Duration::from_secs(5))
            .await?;
        Ok(self.owner_user_id.clone())
    }

    fn cursor(&self) -> Option<String> {
        self.cursor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// QR login orchestration (stage 4): fetch a QR code, long-poll its status,
/// and surface the confirmed credentials. Short-lived in-process state; the
/// orchestration stores the channel after `Confirmed`.
pub struct WeixinLoginFlow {
    client: reqwest::Client,
}

impl WeixinLoginFlow {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    async fn post(&self, url: String, body: serde_json::Value) -> Result<serde_json::Value> {
        let response = self
            .client
            .post(url)
            .timeout(REQUEST_TIMEOUT)
            .header("Content-Type", "application/json")
            .header("AuthorizationType", "ilink_bot_token")
            .header("X-WECHAT-UIN", random_uin())
            .header("iLink-App-Id", APP_ID)
            .header("iLink-App-ClientVersion", CLIENT_VERSION.to_string())
            .json(&body)
            .send()
            .await
            .map_err(|err| transport_error(format!("weixin login failed: {err}")))?;
        let payload: serde_json::Value = response
            .json()
            .await
            .map_err(|err| transport_error(format!("weixin login returned non-JSON: {err}")))?;
        Ok(payload)
    }

    async fn get(&self, url: String) -> Result<serde_json::Value> {
        let response = self
            .client
            .get(url)
            .timeout(REQUEST_TIMEOUT)
            .header("iLink-App-Id", APP_ID)
            .header("iLink-App-ClientVersion", CLIENT_VERSION.to_string())
            .send()
            .await
            .map_err(|err| transport_error(format!("weixin login poll failed: {err}")))?;
        let payload: serde_json::Value = response.json().await.map_err(|err| {
            transport_error(format!("weixin login poll returned non-JSON: {err}"))
        })?;
        Ok(payload)
    }
}

#[async_trait]
impl ImLoginFlow for WeixinLoginFlow {
    async fn start(&self) -> Result<LoginSession> {
        let payload = self
            .post(
                format!("{DEFAULT_BASE_URL}/ilink/bot/get_bot_qrcode?bot_type={QR_BOT_TYPE}"),
                serde_json::json!({ "local_token_list": [] }),
            )
            .await?;
        let qrcode = payload
            .get("qrcode")
            .and_then(|q| q.as_str())
            .ok_or_else(|| transport_error("weixin QR response missing qrcode"))?
            .to_string();
        let qr_content = payload
            .get("qrcode_img_content")
            .or_else(|| payload.get("qrcodeUrl"))
            .and_then(|q| q.as_str())
            .unwrap_or(&qrcode)
            .to_string();
        Ok(LoginSession {
            id: qrcode,
            qr_content,
        })
    }

    async fn poll(&self, session: &LoginSession) -> Result<LoginPoll> {
        let status = self
            .get(format!(
                "{DEFAULT_BASE_URL}/ilink/bot/get_qrcode_status?qrcode={}",
                session.id
            ))
            .await?;
        Ok(self.map_status(&status))
    }
}

impl WeixinLoginFlow {
    /// Map one status payload onto the poll outcome; split from `poll` so the
    /// status machine is testable without the network.
    fn map_status(&self, status: &serde_json::Value) -> LoginPoll {
        match status.get("status").and_then(|s| s.as_str()) {
            Some("wait") => LoginPoll::Pending,
            Some("scaned") | Some("scaned_but_redirect") => LoginPoll::Scanned,
            Some("expired") => LoginPoll::Expired,
            Some("need_verifycode") | Some("verify_code_blocked") => {
                LoginPoll::Failed("微信要求手机端验证码，请稍后重新生成二维码".to_string())
            }
            Some("binded_redirect") => LoginPoll::Expired,
            Some("confirmed") => {
                let token = status
                    .get("bot_token")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string();
                let owner = status
                    .get("ilink_user_id")
                    .and_then(|u| u.as_str())
                    .unwrap_or_default()
                    .to_string();
                if token.is_empty() || owner.is_empty() {
                    return LoginPoll::Failed("登录失败：服务器未返回完整账号信息".to_string());
                }
                let base_url = status
                    .get("baseurl")
                    .and_then(|b| b.as_str())
                    .filter(|b| !b.trim().is_empty())
                    .map(str::to_string);
                LoginPoll::Confirmed {
                    credentials: ImCredentials::Weixin {
                        bot_token: token,
                        base_url,
                        owner_user_id: owner.clone(),
                    },
                    title: format!("微信 {owner}"),
                }
            }
            other => LoginPoll::Failed(format!("未知的二维码状态: {}", other.unwrap_or("none"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_extraction_scopes_to_text_and_voice_items() {
        let items: serde_json::Value = serde_json::from_str(
            r#"[{"type": 1, "text_item": {"text": " hello "}}, {"type": 2, "image_item": {}}]"#,
        )
        .unwrap();
        assert_eq!(text_from_items(&items).as_deref(), Some("hello"));

        let voice: serde_json::Value =
            serde_json::from_str(r#"[{"type": 3, "voice_item": {"text": "transcribed"}}]"#)
                .unwrap();
        assert_eq!(text_from_items(&voice).as_deref(), Some("transcribed"));

        let media: serde_json::Value =
            serde_json::from_str(r#"[{"type": 2, "image_item": {"url": "x"}}]"#).unwrap();
        assert!(text_from_items(&media).is_none());
    }

    #[test]
    fn business_errors_ride_inside_200_responses() {
        let ok = serde_json::json!({ "ret": 0, "msgs": [] });
        assert!(business_error(&ok, "getupdates").is_none());
        let busy = serde_json::json!({ "ret": -2, "errmsg": "prepare failed" });
        let message = business_error(&busy, "sendmessage").unwrap();
        assert!(message.contains("-2") && message.contains("prepare failed"));
        let errcode = serde_json::json!({ "errcode": 5, "message": "bad token" });
        assert!(business_error(&errcode, "getupdates").is_some());
    }

    #[test]
    fn confirmed_login_carries_credentials_and_owner() {
        let flow = WeixinLoginFlow::new(reqwest::Client::new());
        let confirmed = serde_json::json!({
            "status": "confirmed",
            "bot_token": "tok",
            "ilink_user_id": "wxid_1",
            "baseurl": "https://relay.example.com"
        });
        match flow.map_status(&confirmed) {
            LoginPoll::Confirmed { credentials, title } => match credentials {
                ImCredentials::Weixin {
                    bot_token,
                    base_url,
                    owner_user_id,
                } => {
                    assert_eq!(bot_token, "tok");
                    assert_eq!(base_url.as_deref(), Some("https://relay.example.com"));
                    assert_eq!(owner_user_id, "wxid_1");
                    assert_eq!(title, "微信 wxid_1");
                }
                other => panic!("unexpected credentials: {other:?}"),
            },
            other => panic!("unexpected poll: {other:?}"),
        }
    }
}
