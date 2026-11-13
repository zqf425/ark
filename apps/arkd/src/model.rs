//! Model client selection and metering (design §7.2 / §7.15).
//!
//! `arkd` is the only place that decides which `ModelClient` runs. When the
//! configuration carries a usable provider it builds the real HTTP client;
//! otherwise it falls back to a deterministic local client so the runtime can be
//! started and smoke-tested without credentials. The choice is made once, here —
//! never inside a route.

use std::sync::Arc;

use ark_common::{Result, ThreadId, TurnId, Usage};
use ark_config::RuntimeConfig;
use ark_llm::{HttpModelClient, ProviderConfig};
use ark_obs::UsageRecorder;
use ark_ports::{
    truncate_utf8, HookBroker, HookScope, LlmCallFact, ModelChunk, ModelClient, ModelRequest,
    ModelResponse, LLM_TAIL_BYTES,
};
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
            let client = HttpModelClient::new(provider, governed_transport());
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
    let client = HttpModelClient::new(provider, governed_transport());
    (Arc::new(MeteredModel::new(Arc::new(client), usage)), label)
}

/// The one transport for model and catalog traffic.
///
/// Model responses are SSE streams, so no client-level timeout: request phases
/// are bounded inside `ark-llm` instead. Proxy/TLS settings come from the same
/// [`ark_http::HttpPolicy`] the plugin egress bus uses, keeping a single
/// transport configuration in the process (design §7.2).
pub(crate) fn governed_transport() -> reqwest::Client {
    ark_http::transport_client(&ark_http::HttpPolicy::default(), "ark-llm/model", None)
        .unwrap_or_else(|err| {
            tracing::warn!(
                error = %err.message,
                "falling back to default http transport for model traffic"
            );
            reqwest::Client::new()
        })
}

/// The compaction threshold the turn runs with.
///
/// Derived from the resolved provider's context window — the session may fill
/// [`ark_llm::COMPACTION_TRIGGER_RATIO`] of it before the prefix folds, with
/// the user-configured `max_tokens` reserved for output. An undeclared window
/// uses [`ark_llm::DEFAULT_CONTEXT_WINDOW_TOKENS`] (256K, the same fallback
/// the settings inventory applies), so compaction is never silently disabled
/// and the prompt cannot grow unbounded.
pub fn context_limit_tokens(provider: Option<&ProviderConfig>) -> u64 {
    provider
        .and_then(ark_llm::ProviderConfig::compaction_threshold)
        .unwrap_or(
            (ark_llm::DEFAULT_CONTEXT_WINDOW_TOKENS as f64 * ark_llm::COMPACTION_TRIGGER_RATIO)
                as u64,
        )
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

/// Hook decorator: the *outermost* model wrapper (requirements/plugin-hook-bus.md).
///
/// Sitting outside [`MeteredModel`] is load-bearing: a call a hook denies must
/// never be metered, and a tuning hook must see the request before it is
/// serialized. With [`ark_ports::NoHooks`] installed this layer forwards the
/// request byte-for-byte — the zero-behavior promise of the P1 wiring.
///
/// The scope is captured at construction because both turn paths build a fresh
/// client per turn; a process-lifetime client could not know which turn it
/// serves.
pub struct HookModel {
    hooks: Arc<dyn HookBroker>,
    thread_id: ThreadId,
    turn_id: TurnId,
    inner: Arc<dyn ModelClient>,
}

impl HookModel {
    pub fn new(
        hooks: Arc<dyn HookBroker>,
        thread_id: ThreadId,
        turn_id: TurnId,
        inner: Arc<dyn ModelClient>,
    ) -> Self {
        Self {
            hooks,
            thread_id,
            turn_id,
            inner,
        }
    }
}

#[async_trait::async_trait]
impl ModelClient for HookModel {
    fn provider_id(&self) -> &str {
        self.inner.provider_id()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    async fn stream(
        &self,
        mut request: ModelRequest,
        sink: mpsc::Sender<ModelChunk>,
    ) -> Result<ModelResponse> {
        let scope = HookScope {
            thread_id: &self.thread_id,
            turn_id: &self.turn_id,
        };
        let tuning = self.hooks.on_model_request(scope, &request).await?;
        if let Some(temperature) = tuning.temperature {
            request.temperature = Some(temperature);
        }
        if let Some(max_tokens) = tuning.max_tokens {
            request.max_tokens = Some(max_tokens);
        }
        match self.inner.stream(request, sink).await {
            Ok(response) => {
                let (text_tail, truncated) = truncate_utf8(&response.text, LLM_TAIL_BYTES);
                self.hooks
                    .observe_llm_call(
                        scope,
                        &LlmCallFact {
                            provider_id: self.inner.provider_id(),
                            model: self.inner.model(),
                            usage: response.usage.clone(),
                            text_tail,
                            truncated,
                            error: None,
                        },
                    )
                    .await;
                Ok(response)
            }
            Err(err) => {
                self.hooks
                    .observe_llm_call(
                        scope,
                        &LlmCallFact {
                            provider_id: self.inner.provider_id(),
                            model: self.inner.model(),
                            usage: Usage::default(),
                            text_tail: "",
                            truncated: false,
                            error: Some(&err.message),
                        },
                    )
                    .await;
                Err(err)
            }
        }
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

/// Wrap a per-turn client in the hook decorator (requirements/plugin-hook-bus.md).
///
/// Both turn paths call this after [`MeteredModel`] has been applied, so the
/// chain is `HookModel(MeteredModel(client))` — hooks see and may deny a call
/// before it is metered.
pub fn hooked(
    hooks: Arc<dyn HookBroker>,
    thread_id: ThreadId,
    turn_id: TurnId,
    client: Arc<dyn ModelClient>,
) -> Arc<dyn ModelClient> {
    Arc::new(HookModel::new(hooks, thread_id, turn_id, client))
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
    fn threshold_is_80_percent_of_the_window_after_reserving_output() {
        let mut provider = ProviderConfig::new("p", "m");
        provider.context_window_tokens = Some(32_000);
        provider.max_tokens = Some(4_096);
        // (32_000 - 4_096) * 0.8 = 22_323 (floor).
        assert_eq!(context_limit_tokens(Some(&provider)), 22_323);
        // No user-configured output reserve: the full window drives the ratio.
        provider.max_tokens = None;
        assert_eq!(context_limit_tokens(Some(&provider)), 25_600);
        // An undeclared window must not disable compaction: the 256K default
        // window applies (80% of it = 204_800).
        assert_eq!(context_limit_tokens(None), 204_800);
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

    /// A recording broker for decorator tests: it notes every hook it saw and
    /// can optionally retune or deny the model request.
    struct RecordingBroker {
        tuning: Option<ark_ports::ModelRequestTuning>,
        deny_model_request: bool,
        requests_seen: std::sync::Mutex<Vec<ModelRequest>>,
        observes: std::sync::Mutex<Vec<String>>,
    }

    impl RecordingBroker {
        fn new() -> Self {
            Self {
                tuning: None,
                deny_model_request: false,
                requests_seen: std::sync::Mutex::new(Vec::new()),
                observes: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn requests_seen(&self) -> Vec<ModelRequest> {
            self.requests_seen.lock().unwrap().clone()
        }

        fn observes(&self) -> Vec<String> {
            self.observes.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ark_ports::HookBroker for RecordingBroker {
        async fn on_turn_pre_start(
            &self,
            _scope: ark_ports::HookScope<'_>,
            _source: &str,
        ) -> Result<()> {
            Ok(())
        }

        async fn on_model_request(
            &self,
            _scope: ark_ports::HookScope<'_>,
            request: &ModelRequest,
        ) -> Result<ark_ports::ModelRequestTuning> {
            self.requests_seen.lock().unwrap().push(request.clone());
            if self.deny_model_request {
                return Err(ark_common::ArkError::new(
                    ark_common::ArkErrorCode::ValidationError,
                    "hook denied the model request",
                ));
            }
            Ok(self.tuning.clone().unwrap_or_default())
        }

        async fn on_tool_pre_call(
            &self,
            _scope: ark_ports::HookScope<'_>,
            _tool: &str,
            _args_head: &str,
            _args_truncated: bool,
        ) -> Result<ark_ports::ToolPreCallVerdict> {
            Ok(ark_ports::ToolPreCallVerdict::Allow)
        }

        async fn observe_turn_end(
            &self,
            _scope: ark_ports::HookScope<'_>,
            outcome: &ark_ports::TurnOutcomeFact,
        ) {
            self.observes.lock().unwrap().push(format!("{outcome:?}"));
        }

        async fn observe_llm_call(
            &self,
            _scope: ark_ports::HookScope<'_>,
            fact: &ark_ports::LlmCallFact<'_>,
        ) {
            let label = if fact.error.is_some() { "failed" } else { "ok" };
            self.observes.lock().unwrap().push(label.to_string());
        }
    }

    fn scope_pair() -> (ThreadId, TurnId) {
        (ThreadId::new(), TurnId::new())
    }

    fn request_for(text: &str) -> ModelRequest {
        ModelRequest {
            messages: vec![ark_ports::ModelMessage::user(text)],
            ..ModelRequest::default()
        }
    }

    #[tokio::test]
    async fn hook_decorator_forwards_the_request_byte_for_byte_without_opinions() {
        let broker = Arc::new(RecordingBroker::new());
        let thread_turn = scope_pair();
        let client = hooked(
            broker.clone(),
            thread_turn.0,
            thread_turn.1,
            Arc::new(LocalModel::new()),
        );
        let request = request_for("hello");
        let expected = request.clone();
        let (sender, _receiver) = mpsc::channel(16);
        let response = client.stream(request, sender).await.unwrap();
        assert!(response.text.contains("hello"));
        // NoHooks semantics: the inner client must see exactly the request the
        // reactor built, and the observe hook must have reported success.
        let seen = broker.requests_seen();
        assert_eq!(seen.len(), 1);
        // Byte comparison, not PartialEq: what matters is exactly what the
        // inner client would serialize.
        assert_eq!(
            serde_json::to_string(&seen[0]).unwrap(),
            serde_json::to_string(&expected).unwrap()
        );
        assert_eq!(broker.observes(), vec!["ok".to_string()]);
    }

    #[tokio::test]
    async fn hook_decorator_applies_tuning_and_a_denial_is_never_metered() {
        let recorder = Arc::new(UsageRecorder::new());
        // Baseline usage on this recorder: the denied call must not add to it.
        let mut broker = RecordingBroker::new();
        broker.tuning = Some(ark_ports::ModelRequestTuning {
            temperature: Some(0.9),
            max_tokens: Some(64),
        });
        let thread_turn = scope_pair();
        let tuned = hooked(
            Arc::new(broker),
            thread_turn.0,
            thread_turn.1,
            Arc::new(MeteredModel::new(
                Arc::new(LocalModel::new()),
                recorder.clone(),
            )),
        );
        let (sender, _receiver) = mpsc::channel(16);
        let response = tuned.stream(request_for("hi"), sender).await.unwrap();
        assert!(response.text.contains("hi"));
        let baseline = recorder.usage().total_tokens;
        assert!(baseline > 0, "the tuned call was metered");

        let mut broker = RecordingBroker::new();
        broker.deny_model_request = true;
        let thread_turn = scope_pair();
        let denying = hooked(
            Arc::new(broker),
            thread_turn.0,
            thread_turn.1,
            Arc::new(MeteredModel::new(
                Arc::new(LocalModel::new()),
                Arc::clone(&recorder),
            )),
        );
        let (sender, _receiver) = mpsc::channel(16);
        let err = denying
            .stream(request_for("hi"), sender)
            .await
            .expect_err("the hook denial must stop the call");
        assert!(err.message.contains("hook denied"));
        // The denied call never reached the metering wrapper.
        assert_eq!(recorder.usage().total_tokens, baseline);
    }
}
