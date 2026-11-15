//! Feishu/Lark transport (requirements/im-channels.md, stage 3).
//!
//! The WebSocket long connection follows the official gateway protocol as
//! implemented by `@larksuiteoapi/node-sdk` (the protocol reference this
//! uses), reimplemented here without a Node dependency:
//!
//! 1. `POST {domain}/callback/ws/endpoint` with `{AppID, AppSecret}` returns a
//!    signed WebSocket URL (`device_id`/`service_id` query params) and client
//!    config (`PingInterval`, `ReconnectCount`, `ReconnectInterval`,
//!    `ReconnectNonce`, all seconds).
//! 2. Frames are `pbbp2.Frame` protobuf: `SeqID`(1), `LogID`(2), `service`(3),
//!    `method`(4: 0=control, 1=data), `headers`(5: repeated `{key,value}`),
//!    `payloadEncoding`(6), `payloadType`(7), `payload`(8), `LogIDNew`(9).
//!    The encoding below is a hand-rolled minimal protobuf writer/reader —
//!    only these nine fields exist, so a generated runtime would be weight
//!    without benefit.
//! 3. The client pings every `PingInterval` seconds (control frame, header
//!    `type=ping`); the pong payload re-announces the config values.
//! 4. Data frames carry events chunked across `sum` parts keyed by
//!    `message_id`/`seq`; the merged JSON is dispatched and must be answered
//!    with an echo frame whose payload is base64 of `{"code":200}`.
//!
//! Events are pushed and acknowledged in-band, so there is no replay cursor:
//! cross-restart redelivery is bounded by the gateway's own retry window and
//! the orchestration's dedup set (cursor stays `None`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ark_common::{ArkError, ArkErrorCode, Result};
use ark_contract::{FeishuDomain, ImChannelId, ImChannelKind};
use ark_ports::CancelToken;
use async_trait::async_trait;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::domain::{chunk_text, truncate_inbound, InboundMessage};

const EVENT_CHUNK_TTL: Duration = Duration::from_secs(10);
/// Per-request client deadline for the REST calls (config pull, token, send).
const REST_TIMEOUT: Duration = Duration::from_secs(15);
/// Reconnect floor; the ceiling comes from the gateway's own config.
const RECONNECT_MIN: Duration = Duration::from_secs(2);
const RECONNECT_MAX: Duration = Duration::from_secs(120);

/// The nine-field wire frame. `payload` is raw bytes; headers are ordered.
#[derive(Debug, Default, Clone)]
struct Frame {
    seq_id: u64,
    log_id: u64,
    service: i32,
    method: i32,
    headers: Vec<(String, String)>,
    payload: Vec<u8>,
}

fn write_varint(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            buf.push(byte);
            return;
        }
        buf.push(byte | 0x80);
    }
}

fn read_varint(data: &[u8], offset: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    while *offset < data.len() {
        let byte = data[*offset];
        *offset += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
    None
}

fn encode_frame(frame: &Frame) -> Vec<u8> {
    let mut buf = Vec::new();
    write_varint(&mut buf, 1 << 3);
    write_varint(&mut buf, frame.seq_id);
    write_varint(&mut buf, 2 << 3);
    write_varint(&mut buf, frame.log_id);
    write_varint(&mut buf, 3 << 3);
    write_varint(&mut buf, frame.service as u64);
    write_varint(&mut buf, 4 << 3);
    write_varint(&mut buf, frame.method as u64);
    for (key, value) in &frame.headers {
        write_varint(&mut buf, 5 << 3 | 2);
        let mut header = Vec::new();
        write_varint(&mut header, 1 << 3 | 2);
        write_varint(&mut header, key.len() as u64);
        header.extend_from_slice(key.as_bytes());
        write_varint(&mut header, 2 << 3 | 2);
        write_varint(&mut header, value.len() as u64);
        header.extend_from_slice(value.as_bytes());
        write_varint(&mut buf, header.len() as u64);
        buf.extend_from_slice(&header);
    }
    if !frame.payload.is_empty() {
        write_varint(&mut buf, 8 << 3 | 2);
        write_varint(&mut buf, frame.payload.len() as u64);
        buf.extend_from_slice(&frame.payload);
    }
    buf
}

fn decode_frame(data: &[u8]) -> Option<Frame> {
    let mut frame = Frame::default();
    let mut offset = 0usize;
    while offset < data.len() {
        let tag = read_varint(data, &mut offset)?;
        let field = (tag >> 3) as u32;
        let wire = tag & 0x7;
        match (field, wire) {
            (1, 0) => frame.seq_id = read_varint(data, &mut offset)?,
            (2, 0) => frame.log_id = read_varint(data, &mut offset)?,
            (3, 0) => frame.service = read_varint(data, &mut offset)? as i32,
            (4, 0) => frame.method = read_varint(data, &mut offset)? as i32,
            (5, 2) => {
                let len = read_varint(data, &mut offset)? as usize;
                let end = offset.checked_add(len)?;
                let header = data.get(offset..end)?;
                offset = end;
                let mut key = String::new();
                let mut value = String::new();
                let mut h = 0usize;
                while h < header.len() {
                    let tag = read_varint(header, &mut h)?;
                    let flen = read_varint(header, &mut h)? as usize;
                    let bytes = header.get(h..h + flen)?;
                    h += flen;
                    match tag >> 3 {
                        1 => key = String::from_utf8_lossy(bytes).into_owned(),
                        2 => value = String::from_utf8_lossy(bytes).into_owned(),
                        _ => {}
                    }
                }
                frame.headers.push((key, value));
            }
            (8, 2) => {
                let len = read_varint(data, &mut offset)? as usize;
                let end = offset.checked_add(len)?;
                frame.payload = data.get(offset..end)?.to_vec();
                offset = end;
            }
            // Unknown fields must be skipped, not rejected: the gateway may
            // send fields (payloadEncoding, payloadType, LogIDNew) we do not
            // model.
            (_, 0) => {
                read_varint(data, &mut offset)?;
            }
            (_, 2) => {
                let len = read_varint(data, &mut offset)? as usize;
                offset = offset.checked_add(len)?;
            }
            _ => return None,
        }
    }
    Some(frame)
}

fn header<'a>(frame: &'a Frame, key: &str) -> Option<&'a str> {
    frame
        .headers
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

type ChunkedEvent = (Vec<Option<Vec<u8>>>, Instant);

/// Assembles chunked event payloads (`sum` parts per `message_id`).
#[derive(Default)]
struct ChunkCache {
    parts: HashMap<String, ChunkedEvent>,
}

impl ChunkCache {
    fn merge(&mut self, message_id: &str, sum: usize, seq: usize, data: &[u8]) -> Option<Vec<u8>> {
        let entry = self
            .parts
            .entry(message_id.to_string())
            .or_insert_with(|| (vec![None; sum.max(1)], Instant::now()));
        if entry.0.len() != sum.max(1) {
            entry.0.resize(sum.max(1), None);
        }
        let slot = seq.min(entry.0.len() - 1);
        entry.0[slot] = Some(data.to_vec());
        if entry.0.iter().all(|part| part.is_some()) {
            let merged: Vec<u8> = entry
                .0
                .iter()
                .filter_map(|part| part.as_deref())
                .flatten()
                .copied()
                .collect();
            self.parts.remove(message_id);
            return Some(merged);
        }
        None
    }

    fn expire(&mut self) {
        self.parts
            .retain(|_, (_, created)| created.elapsed() < EVENT_CHUNK_TTL);
    }
}

fn transport_error(message: impl Into<String>) -> ArkError {
    ArkError::new(ArkErrorCode::ToolExecutionFailed, message)
}

pub struct FeishuTransport {
    client: reqwest::Client,
    app_id: String,
    app_secret: String,
    base: String,
    bot_open_id: Mutex<Option<String>>,
    /// Feishu pushes events with in-band acks; no replay cursor exists.
    cursor: Mutex<Option<String>>,
    token: Mutex<Option<(String, Instant)>>,
}

impl FeishuTransport {
    pub fn new(
        client: reqwest::Client,
        app_id: String,
        app_secret: String,
        domain: FeishuDomain,
    ) -> Self {
        let base = match domain {
            FeishuDomain::Feishu => "https://open.feishu.cn",
            FeishuDomain::Lark => "https://open.larksuite.com",
        }
        .to_string();
        Self {
            client,
            app_id,
            app_secret,
            base,
            bot_open_id: Mutex::new(None),
            cursor: Mutex::new(None),
            token: Mutex::new(None),
        }
    }

    async fn call_api(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        let mut request = self
            .client
            .post(format!("{}{}", self.base, path))
            .timeout(REST_TIMEOUT)
            .json(&body);
        if let Some((token, _)) = self
            .token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|err| transport_error(format!("feishu {path} failed: {err}")))?;
        let payload: serde_json::Value = response
            .json()
            .await
            .map_err(|err| transport_error(format!("feishu {path} returned non-JSON: {err}")))?;
        Ok(payload)
    }

    /// Tenant access token with in-memory caching; refreshed a minute early.
    async fn tenant_token(&self) -> Result<String> {
        {
            let cached = self
                .token
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((token, fetched_at)) = cached.as_ref() {
                if fetched_at.elapsed() < Duration::from_secs(55 * 60) {
                    return Ok(token.clone());
                }
            }
        }
        let payload = self
            .call_api(
                "/open-apis/auth/v3/tenant_access_token/internal",
                serde_json::json!({ "app_id": self.app_id, "app_secret": self.app_secret }),
            )
            .await?;
        let token = payload
            .get("tenant_access_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| transport_error("feishu token response missing tenant_access_token"))?
            .to_string();
        *self
            .token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((token.clone(), Instant::now()));
        Ok(token)
    }

    async fn pull_connect_config(&self) -> Result<(String, serde_json::Value)> {
        // The gateway reads the capitalized keys and answers with the signed
        // URL plus client config; code 0 means ok (see protocol notes above).
        let response = self
            .client
            .post(format!("{}/callback/ws/endpoint", self.base))
            .timeout(REST_TIMEOUT)
            .header("locale", "zh")
            .header("User-Agent", "ark-im/feishu")
            .json(&serde_json::json!({
                "AppID": self.app_id,
                "AppSecret": self.app_secret,
            }))
            .send()
            .await
            .map_err(|err| transport_error(format!("feishu ws endpoint failed: {err}")))?;
        let payload: serde_json::Value = response.json().await.map_err(|err| {
            transport_error(format!("feishu ws endpoint returned non-JSON: {err}"))
        })?;
        if payload.get("code").and_then(|c| c.as_i64()) != Some(0) {
            return Err(transport_error(format!(
                "feishu ws endpoint error: {}",
                payload
                    .get("msg")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown")
            )));
        }
        let url = payload
            .pointer("/data/URL")
            .and_then(|u| u.as_str())
            .ok_or_else(|| transport_error("feishu ws endpoint response missing URL"))?
            .to_string();
        let client_config = payload
            .pointer("/data/ClientConfig")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Ok((url, client_config))
    }

    fn config_u64(config: &serde_json::Value, key: &str, fallback: u64) -> u64 {
        config.get(key).and_then(|v| v.as_u64()).unwrap_or(fallback)
    }

    async fn ensure_bot_identity(&self) {
        if self
            .bot_open_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
        {
            return;
        }
        if let Ok(info) = self
            .call_api("/open-apis/bot/v3/info", serde_json::json!({}))
            .await
        {
            let open_id = info
                .pointer("/bot/open_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            *self
                .bot_open_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = open_id;
        }
    }

    /// Fold one `im.message.receive_v1` event into the normalized shape, or
    /// `None` for everything v1 cannot act on (non-text, other event types).
    fn normalize_event(&self, event: &serde_json::Value) -> Option<InboundMessage> {
        let message = event.pointer("/event/message")?;
        if message.get("message_type").and_then(|t| t.as_str()) != Some("text") {
            return None;
        }
        let content: serde_json::Value =
            serde_json::from_str(message.get("content").and_then(|c| c.as_str())?).ok()?;
        let text = content.get("text").and_then(|t| t.as_str())?;
        let chat_type = message.get("chat_type").and_then(|t| t.as_str())?;
        let sender_id = event
            .pointer("/event/sender/sender_id/open_id")
            .or_else(|| event.pointer("/event/sender/sender_id/union_id"))
            .or_else(|| event.pointer("/event/sender/sender_id/user_id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        // The receive_v1 event carries no display name; the chat UI shows it.
        Some(InboundMessage {
            channel_id: ImChannelId(String::new()),
            chat_id: message
                .get("chat_id")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_string(),
            sender_id,
            sender_name: String::new(),
            message_id: message
                .get("message_id")
                .and_then(|m| m.as_str())
                .unwrap_or_default()
                .to_string(),
            text: truncate_inbound(text),
            is_dm: chat_type == "p2p",
            is_mention: message
                .get("mentions")
                .and_then(|m| m.as_array())
                .is_some_and(|mentions| !mentions.is_empty()),
        })
    }

    /// One WebSocket session; returns on clean shutdown (`Ok`) or a dropped
    /// connection (`Err`), letting [`FeishuTransport::run`] own the reconnect
    /// cadence.
    async fn run_session(
        &self,
        url: &str,
        ping_interval: u64,
        inbound: &mpsc::Sender<InboundMessage>,
        shutdown: &CancelToken,
    ) -> Result<()> {
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|err| transport_error(format!("feishu ws connect failed: {err}")))?;
        let (mut sink, mut stream) = ws.split();
        let mut chunks = ChunkCache::default();
        let mut ping = tokio::time::interval(Duration::from_secs(ping_interval));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    let _ = sink.close().await;
                    return Ok(());
                }
                _ = ping.tick() => {
                    let frame = Frame {
                        method: 0, // control
                        headers: vec![("type".into(), "ping".into())],
                        ..Frame::default()
                    };
                    sink.send(Message::Binary(encode_frame(&frame)))
                        .await
                        .map_err(|err| transport_error(format!("feishu ping send failed: {err}")))?;
                }
                message = stream.next() => {
                    let Some(message) = message else {
                        return Err(transport_error("feishu ws stream closed"));
                    };
                    let Message::Binary(bytes) = message
                        .map_err(|err| transport_error(format!("feishu ws error: {err}")))?
                    else {
                        continue;
                    };
                    let Some(frame) = decode_frame(&bytes) else {
                        continue;
                    };
                    match frame.method {
                        0 => {
                            // Control: the pong payload re-announces config
                            // values; the endpoint-config cadence already in
                            // use stays authoritative for this session.
                        }
                        1 => {
                            let message_id =
                                header(&frame, "message_id").unwrap_or_default().to_string();
                            let sum: usize = header(&frame, "sum")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(1);
                            let seq: usize = header(&frame, "seq")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0);
                            if header(&frame, "type").unwrap_or_default() != "event" {
                                continue;
                            }
                            chunks.expire();
                            let Some(merged) =
                                chunks.merge(&message_id, sum, seq, &frame.payload)
                            else {
                                // Partial chunk: the ack frame is only sent
                                // once the whole event is assembled, matching
                                // the reference client.
                                continue;
                            };
                            let event: serde_json::Value =
                                serde_json::from_slice(&merged).unwrap_or(serde_json::Value::Null);
                            if let Some(inbound_message) = self.normalize_event(&event) {
                                if inbound.send(inbound_message).await.is_err() {
                                    return Ok(());
                                }
                            }
                            // Echo ack: same headers plus biz_rt, payload is
                            // base64 of `{"code":200}`.
                            let ack_payload = base64::engine::general_purpose::STANDARD
                                .encode(br#"{"code":200}"#);
                            let mut ack = frame.clone();
                            ack.headers.push(("biz_rt".into(), "0".into()));
                            ack.payload = ack_payload.into_bytes();
                            sink.send(Message::Binary(encode_frame(&ack)))
                                .await
                                .map_err(|err| {
                                    transport_error(format!("feishu ack send failed: {err}"))
                                })?;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[async_trait]
impl crate::ImTransport for FeishuTransport {
    fn kind(&self) -> ImChannelKind {
        ImChannelKind::Feishu
    }

    async fn run(
        &self,
        inbound: mpsc::Sender<InboundMessage>,
        shutdown: CancelToken,
    ) -> Result<()> {
        self.ensure_bot_identity().await;
        let mut backoff = RECONNECT_MIN;
        loop {
            if shutdown.is_cancelled() {
                return Ok(());
            }
            let (url, client_config) = match self.pull_connect_config().await {
                Ok(config) => config,
                Err(err) => {
                    // Bad credentials repeat forever; surface the error and
                    // let the orchestration's error cooldown pace the retries.
                    tracing::warn!(error = %err.message, "feishu connect config failed");
                    return Err(err);
                }
            };
            let ping_interval = Self::config_u64(&client_config, "PingInterval", 120).max(10);
            match self
                .run_session(&url, ping_interval, &inbound, &shutdown)
                .await
            {
                Ok(()) => return Ok(()), // shutdown
                Err(err) => {
                    tracing::warn!(error = %err.message, "feishu ws session ended; reconnecting");
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = shutdown.cancelled() => return Ok(()),
            }
            backoff = (backoff * 2).min(RECONNECT_MAX);
        }
    }

    async fn send_text(&self, chat_id: &str, text: &str) -> Result<()> {
        let token = self.tenant_token().await?;
        for chunk in chunk_text(self.kind(), text) {
            let content = serde_json::json!({ "text": chunk }).to_string();
            let response = self
                .client
                .post(format!(
                    "{}/open-apis/im/v1/messages?receive_id_type=chat_id",
                    self.base
                ))
                .timeout(REST_TIMEOUT)
                .bearer_auth(&token)
                .json(&serde_json::json!({
                    "receive_id": chat_id,
                    "msg_type": "text",
                    "content": content,
                }))
                .send()
                .await
                .map_err(|err| transport_error(format!("feishu send failed: {err}")))?;
            let payload: serde_json::Value = response
                .json()
                .await
                .map_err(|err| transport_error(format!("feishu send returned non-JSON: {err}")))?;
            if payload.get("code").and_then(|c| c.as_i64()) != Some(0) {
                return Err(transport_error(format!(
                    "feishu send error: {}",
                    payload
                        .get("msg")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown")
                )));
            }
        }
        Ok(())
    }

    async fn verify(&self) -> Result<String> {
        let info = self
            .call_api("/open-apis/bot/v3/info", serde_json::json!({}))
            .await?;
        let name = info
            .pointer("/bot/app_name")
            .and_then(|n| n.as_str())
            .unwrap_or("unknown");
        Ok(name.to_string())
    }

    fn cursor(&self) -> Option<String> {
        self.cursor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn is_self(&self, sender_id: &str) -> bool {
        self.bot_open_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_deref()
            == Some(sender_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip_survives_headers_and_payload() {
        let frame = Frame {
            seq_id: 7,
            log_id: 9,
            service: 3,
            method: 1,
            headers: vec![
                ("type".into(), "event".into()),
                ("message_id".into(), "om_1".into()),
                ("sum".into(), "2".into()),
                ("seq".into(), "1".into()),
            ],
            payload: b"hello".to_vec(),
        };
        let decoded = decode_frame(&encode_frame(&frame)).expect("decodable");
        assert_eq!(decoded.seq_id, 7);
        assert_eq!(decoded.service, 3);
        assert_eq!(decoded.method, 1);
        assert_eq!(decoded.headers, frame.headers);
        assert_eq!(decoded.payload, b"hello");
    }

    #[test]
    fn unknown_fields_are_skipped_not_rejected() {
        // payloadEncoding(6) is a length-delimited field we do not model; a
        // frame carrying it must still decode.
        let mut buf = Vec::new();
        write_varint(&mut buf, 6 << 3 | 2);
        write_varint(&mut buf, 3);
        buf.extend_from_slice(b"raw");
        write_varint(&mut buf, 4 << 3);
        write_varint(&mut buf, 1);
        let decoded = decode_frame(&buf).expect("decodable");
        assert_eq!(decoded.method, 1);
    }

    #[test]
    fn chunk_cache_merges_in_seq_order() {
        let mut cache = ChunkCache::default();
        assert!(cache.merge("m", 2, 1, b"world").is_none());
        assert_eq!(
            cache.merge("m", 2, 0, b"hello ").as_deref(),
            Some(b"hello world".as_slice())
        );
        assert!(cache.parts.is_empty(), "completed events are dropped");
    }

    #[test]
    fn event_normalization_scopes_to_text_messages() {
        let transport = FeishuTransport::new(
            reqwest::Client::new(),
            "app".into(),
            "secret".into(),
            FeishuDomain::Feishu,
        );
        let event: serde_json::Value = serde_json::from_str(
            r#"{
                "event": {
                    "message": {
                        "chat_id": "oc_1",
                        "chat_type": "group",
                        "message_type": "text",
                        "message_id": "om_1",
                        "content": "{\"text\":\"hi\"}",
                        "mentions": [{"key": "@_user_1", "id": {"open_id": "ou_bot"}}]
                    },
                    "sender": {"sender_id": {"open_id": "ou_u1"}, "sender_type": "user"}
                }
            }"#,
        )
        .unwrap();
        let normalized = transport.normalize_event(&event).expect("normalized");
        assert_eq!(normalized.chat_id, "oc_1");
        assert!(!normalized.is_dm);
        assert!(normalized.is_mention);
        assert_eq!(normalized.text, "hi");

        let p2p: serde_json::Value = serde_json::from_str(
            r#"{
                "event": {
                    "message": {"chat_id": "oc_2", "chat_type": "p2p", "message_type": "text",
                                "message_id": "om_2", "content": "{\"text\":\"dm\"}"},
                    "sender": {"sender_id": {"open_id": "ou_u1"}}
                }
            }"#,
        )
        .unwrap();
        let normalized = transport.normalize_event(&p2p).expect("normalized");
        assert!(normalized.is_dm);
        assert!(!normalized.is_mention);

        let image: serde_json::Value = serde_json::from_str(
            r#"{
                "event": {
                    "message": {"chat_id": "oc_1", "chat_type": "p2p", "message_type": "image",
                                "message_id": "om_3", "content": "{\"image_key\":\"k\"}"},
                    "sender": {"sender_id": {"open_id": "ou_u1"}}
                }
            }"#,
        )
        .unwrap();
        assert!(
            transport.normalize_event(&image).is_none(),
            "media is out of scope"
        );
    }
}
