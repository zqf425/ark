//! Model client selection and metering (design §7.2 / §7.15).
//!
//! `arkd` is the only place that decides which `ModelClient` runs. When the
//! configuration carries a usable provider it builds the real HTTP client;
//! otherwise it falls back to a deterministic local client so the runtime can be
//! started and smoke-tested without credentials. The choice is made once, here —
//! never inside a route.

use std::sync::Arc;

use ark_common::{Result, Usage};
use ark_config::RuntimeConfig;
use ark_llm::{HttpModelClient, ProviderConfig};
use ark_obs::UsageRecorder;
use ark_ports::{ModelChunk, ModelClient, ModelRequest, ModelResponse};
use tokio::sync::mpsc;

/// Provider table keys understood by [`select`].
pub const KEY_ENABLED: &str = "enabled";
pub const KEY_MODEL: &str = "model";
pub const KEY_PROVIDER_ID: &str = "provider_id";
pub const KEY_API_KEY: &str = "api_key";
pub const KEY_BASE_URL: &str = "base_url";

/// Build the model client the runtime should use.
///
/// Returns `(client, description)` so `arkd` can print exactly which one is live.
pub fn select(config: &RuntimeConfig, usage: Arc<UsageRecorder>) -> (Arc<dyn ModelClient>, String) {
    match http_provider(config) {
        Some(provider) => {
            let label = format!(
                "http provider `{}` model `{}`",
                provider.provider_id, provider.model
            );
            let client = HttpModelClient::with_config(provider);
            (Arc::new(MeteredModel::new(Arc::new(client), usage)), label)
        }
        None => {
            let label = "local deterministic model (no provider configured)".to_string();
            (
                Arc::new(MeteredModel::new(Arc::new(LocalModel::new()), usage)),
                label,
            )
        }
    }
}

/// Build a client from a provider-table entry.
///
/// This is the path the GUI uses: a provider added or edited through
/// `/v1/runtime/providers` is resolved from `providers.json`, so the next turn
/// picks it up without a restart (design §7.15: the provider table is `Hot`).
pub fn select_from_config(
    provider: ProviderConfig,
    usage: Arc<UsageRecorder>,
) -> (Arc<dyn ModelClient>, String) {
    let label = format!(
        "http provider `{}` model `{}`",
        provider.provider_id, provider.model
    );
    let client = HttpModelClient::with_config(provider);
    (Arc::new(MeteredModel::new(Arc::new(client), usage)), label)
}

/// Cap a configured compaction threshold to the selected model's known window.
///
/// Parameter: `configured` is the runtime-wide threshold; `provider` supplies
/// optional catalog metadata and requested output tokens. Returns a positive
/// threshold that reserves requested output only when the user configured one.
pub fn context_limit_tokens(configured: u64, provider: Option<&ProviderConfig>) -> u64 {
    let configured = configured.max(1);
    provider
        .and_then(|provider| {
            provider
                .context_window_tokens
                .map(|window| (provider, window))
        })
        .map(|(provider, window)| {
            let output_reserve = provider.max_tokens.map(u64::from).unwrap_or(0);
            window.saturating_sub(output_reserve).max(1)
        })
        .map(|model_limit| configured.min(model_limit))
        .unwrap_or(configured)
}

/// Extract a usable provider from the opaque `model_providers` value.
///
/// Accepted shapes: `{"provider_id","model","api_key","base_url"}` or an array
/// whose first entry is such an object. Anything else is "not configured".
fn http_provider(config: &RuntimeConfig) -> Option<ProviderConfig> {
    let value = &config.model_providers;
    let object = match value {
        serde_json::Value::Object(map) => map,
        serde_json::Value::Array(items) => items.first()?.as_object()?,
        _ => return None,
    };
    if object.get(KEY_ENABLED).and_then(|flag| flag.as_bool()) == Some(false) {
        return None;
    }
    let model = object.get(KEY_MODEL)?.as_str()?.trim().to_string();
    if model.is_empty() {
        return None;
    }
    let api_key = object
        .get(KEY_API_KEY)
        .and_then(|key| key.as_str())
        .unwrap_or_default()
        .to_string();
    if api_key.trim().is_empty() {
        // Without a credential the HTTP client could only fail; stay local.
        return None;
    }
    let mut provider = ProviderConfig::new(
        object
            .get(KEY_PROVIDER_ID)
            .and_then(|id| id.as_str())
            .unwrap_or("custom"),
        model,
    );
    provider.api_key = api_key;
    provider.base_url = object
        .get(KEY_BASE_URL)
        .and_then(|url| url.as_str())
        .unwrap_or_default()
        .to_string();
    Some(provider)
}

/// Wraps a client and folds every provider-reported sample into the recorder.
///
/// The recorder is the only place usage accumulates, so `/v1/usage` can never
/// report a number the provider did not send.
pub struct MeteredModel {
    inner: Arc<dyn ModelClient>,
    usage: Arc<UsageRecorder>,
}

impl MeteredModel {
    pub fn new(inner: Arc<dyn ModelClient>, usage: Arc<UsageRecorder>) -> Self {
        Self { inner, usage }
    }
}

#[async_trait::async_trait]
impl ModelClient for MeteredModel {
    fn provider_id(&self) -> &str {
        self.inner.provider_id()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    async fn stream(
        &self,
        request: ModelRequest,
        sink: mpsc::Sender<ModelChunk>,
    ) -> Result<ModelResponse> {
        let response = self.inner.stream(request, sink).await?;
        self.usage.record(&response.usage);
        Ok(response)
    }
}

/// Deterministic local client: streams a short answer built from the last user
/// message, so a turn always produces real events without a provider.
///
/// This is a development fallback, and `arkd` says so in its startup line. It is
/// not a fake provider pretending to be a model: it reports its own usage and its
/// own id.
pub struct LocalModel {
    provider_id: String,
    model: String,
}

impl Default for LocalModel {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalModel {
    pub fn new() -> Self {
        Self {
            provider_id: "ark-local".to_string(),
            model: "ark-local-echo".to_string(),
        }
    }
}

#[async_trait::async_trait]
impl ModelClient for LocalModel {
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn stream(
        &self,
        request: ModelRequest,
        sink: mpsc::Sender<ModelChunk>,
    ) -> Result<ModelResponse> {
        let prompt = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == ark_ports::ModelRole::User)
            .map(|message| message.content.clone())
            .unwrap_or_default();
        let reply = format!("ark-local received: {prompt}");
        for word in reply.split_inclusive(' ') {
            if sink
                .send(ModelChunk::TextDelta {
                    delta: word.to_string(),
                })
                .await
                .is_err()
            {
                return Err(ark_common::ArkError::new(
                    ark_common::ArkErrorCode::RuntimeUnavailable,
                    "sink closed",
                ));
            }
        }
        let usage = Usage::new(estimate_tokens(&prompt), estimate_tokens(&reply));
        let _ = sink
            .send(ModelChunk::Usage {
                usage: usage.clone(),
            })
            .await;
        Ok(ModelResponse {
            text: reply,
            tool_calls: Vec::new(),
            usage,
        })
    }
}

/// Rough token estimate (4 bytes per token) for the local client's usage sample.
fn estimate_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_config::RuntimeConfig;

    fn config_with(value: serde_json::Value) -> RuntimeConfig {
        RuntimeConfig {
            model_providers: value,
            ..RuntimeConfig::default()
        }
    }

    #[test]
    fn no_provider_falls_back_to_the_local_client() {
        let (_client, label) = select(&RuntimeConfig::default(), Arc::new(UsageRecorder::new()));
        assert!(label.contains("local deterministic"));
    }

    #[test]
    fn a_complete_provider_selects_http() {
        let config = config_with(serde_json::json!({
            "provider_id": "deepseek",
            "model": "deepseek-chat",
            "api_key": "sk-test",
            "base_url": "https://api.deepseek.com"
        }));
        let (_client, label) = select(&config, Arc::new(UsageRecorder::new()));
        assert!(label.contains("deepseek-chat"));
    }

    #[test]
    fn disabled_or_keyless_providers_stay_local() {
        let disabled = config_with(serde_json::json!({
            "enabled": false, "model": "x", "api_key": "sk"
        }));
        assert!(http_provider(&disabled).is_none());
        let keyless = config_with(serde_json::json!({ "model": "x" }));
        assert!(http_provider(&keyless).is_none());
        let array = config_with(serde_json::json!([{ "model": "m", "api_key": "sk" }]));
        assert!(http_provider(&array).is_some());
    }

    #[test]
    fn explicit_model_window_caps_context_after_reserving_output() {
        let mut provider = ProviderConfig::new("p", "m");
        provider.context_window_tokens = Some(32_000);
        provider.max_tokens = Some(4_096);
        assert_eq!(context_limit_tokens(100_000, Some(&provider)), 27_904);
        assert_eq!(context_limit_tokens(20_000, Some(&provider)), 20_000);
        assert_eq!(context_limit_tokens(20_000, None), 20_000);
    }

    #[tokio::test]
    async fn local_model_streams_and_reports_usage() {
        let client = LocalModel::new();
        let (sender, mut receiver) = mpsc::channel(16);
        let request = ModelRequest {
            messages: vec![ark_ports::ModelMessage::user("hello")],
            ..ModelRequest::default()
        };
        let response = client.stream(request, sender).await.unwrap();
        assert!(response.text.contains("hello"));
        assert!(response.usage.total_tokens > 0);
        let mut chunks = 0;
        while let Ok(_chunk) = receiver.try_recv() {
            chunks += 1;
        }
        assert!(chunks > 1, "deltas must be streamed, not buffered");
    }

    #[test]
    fn metering_forwards_identity() {
        let metered =
            MeteredModel::new(Arc::new(LocalModel::new()), Arc::new(UsageRecorder::new()));
        assert_eq!(metered.provider_id(), "ark-local");
        assert_eq!(metered.model(), "ark-local-echo");
    }
}
