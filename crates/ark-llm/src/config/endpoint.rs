//! Endpoint families and per-provider settings.
//!
//! Design §7.2: `endpointFormat` decides which provider module handles a
//! request. It affects URL construction, headers, body shape, stream parsing and
//! usage parsing, so it lives on the config and is matched exactly once.

use ark_common::{ArkError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The three supported wire families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EndpointFormat {
    /// `POST {base}/chat/completions` with an SSE `delta` stream.
    #[default]
    OpenAiChat,
    /// `POST {base}/responses` with typed `response.*` events.
    OpenAiResponses,
    /// `POST {base}/messages` with `content_block_delta` events.
    AnthropicMessages,
}

impl EndpointFormat {
    /// Stable identifier used in logs and usage records.
    pub fn as_str(self) -> &'static str {
        match self {
            EndpointFormat::OpenAiChat => "openai_chat",
            EndpointFormat::OpenAiResponses => "openai_responses",
            EndpointFormat::AnthropicMessages => "anthropic_messages",
        }
    }

    /// Default base URL when the user leaves it blank. Matches the TypeScript
    /// runtime's `DEFAULT_DEEPSEEK_BASE_URL` for the OpenAI families.
    pub fn default_base_url(self) -> &'static str {
        match self {
            EndpointFormat::OpenAiChat | EndpointFormat::OpenAiResponses => {
                "https://api.deepseek.com"
            }
            EndpointFormat::AnthropicMessages => "https://api.anthropic.com",
        }
    }

    /// Parse the settings value. Unknown formats are a validation error rather
    /// than a silent fallback, so a typo cannot silently switch wire families.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim() {
            "" => Ok(EndpointFormat::default()),
            "openai_chat" | "openai-chat" | "chat" | "chat_completions" => Ok(EndpointFormat::OpenAiChat),
            "openai_responses" | "responses" => Ok(EndpointFormat::OpenAiResponses),
            "anthropic" | "anthropic_messages" | "messages" => Ok(EndpointFormat::AnthropicMessages),
            other => Err(ArkError::validation(format!(
                "unknown endpoint format `{other}`; expected openai_chat, openai_responses or anthropic_messages"
            ))),
        }
    }
}

/// Everything a provider module needs. Resolved once at construction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Stable provider id, e.g. `deepseek`. Used in logs and usage records.
    pub provider_id: String,
    /// Model name sent on the wire.
    pub model: String,
    /// Base URL. Blank falls back to [`EndpointFormat::default_base_url`].
    #[serde(default)]
    pub base_url: String,
    /// Bearer token / API key.
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub endpoint_format: EndpointFormat,
    /// When set, this is used verbatim as the request URL. A custom full path is
    /// an explicit mode: no provider path is appended (design §7.2).
    #[serde(default)]
    pub custom_endpoint: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Additional provider-specific headers. Core auth and content headers are
    /// applied afterwards and cannot be overridden by this map.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Total model context window when a provider catalog explicitly reported
    /// it. This is used to cap the agent's compaction threshold.
    #[serde(default)]
    pub context_window_tokens: Option<u64>,
}

impl ProviderConfig {
    pub fn new(provider_id: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model: model.into(),
            base_url: String::new(),
            api_key: String::new(),
            endpoint_format: EndpointFormat::default(),
            custom_endpoint: None,
            max_tokens: None,
            temperature: None,
            headers: BTreeMap::new(),
            context_window_tokens: None,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = api_key.into();
        self
    }

    pub fn with_format(mut self, format: EndpointFormat) -> Self {
        self.endpoint_format = format;
        self
    }

    /// Attach user-configured, non-core headers to each provider request.
    ///
    /// Parameter: `headers` is the provider-specific map after validation.
    /// Returns: the updated configuration builder.
    pub fn with_headers(mut self, headers: BTreeMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    /// The base URL actually used, with the blank-value fallback applied.
    pub fn resolved_base_url(&self) -> String {
        let trimmed = self.base_url.trim();
        if trimmed.is_empty() {
            self.endpoint_format.default_base_url().to_string()
        } else {
            trimmed.to_string()
        }
    }

    /// The request URL for one call.
    pub fn request_url(&self) -> String {
        if let Some(custom) = self.custom_endpoint.as_deref().map(str::trim) {
            if !custom.is_empty() {
                return custom.trim_end_matches('/').to_string();
            }
        }
        let base = self.resolved_base_url();
        match self.endpoint_format {
            EndpointFormat::OpenAiChat => crate::config::url::openai_chat_url(&base),
            EndpointFormat::OpenAiResponses => crate::config::url::openai_responses_url(&base),
            EndpointFormat::AnthropicMessages => crate::config::url::anthropic_messages_url(&base),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_base_url_falls_back_to_the_provider_default() {
        let config = ProviderConfig::new("deepseek", "deepseek-chat");
        assert_eq!(config.resolved_base_url(), "https://api.deepseek.com");
        let anthropic = ProviderConfig::new("anthropic", "claude")
            .with_format(EndpointFormat::AnthropicMessages);
        assert_eq!(anthropic.resolved_base_url(), "https://api.anthropic.com");
    }

    #[test]
    fn format_parsing_accepts_aliases() {
        assert_eq!(
            EndpointFormat::parse("").unwrap(),
            EndpointFormat::OpenAiChat
        );
        assert_eq!(
            EndpointFormat::parse("chat").unwrap(),
            EndpointFormat::OpenAiChat
        );
        assert_eq!(
            EndpointFormat::parse("responses").unwrap(),
            EndpointFormat::OpenAiResponses
        );
        assert_eq!(
            EndpointFormat::parse("anthropic").unwrap(),
            EndpointFormat::AnthropicMessages
        );
    }

    #[test]
    fn unknown_format_is_a_validation_error() {
        let error = EndpointFormat::parse("grpc").unwrap_err();
        assert!(error.message.contains("unknown endpoint format"));
    }

    #[test]
    fn custom_endpoint_is_used_verbatim() {
        let config = ProviderConfig::new("x", "m")
            .with_base_url("https://example.com/v1")
            .with_format(EndpointFormat::OpenAiChat);
        let mut custom = config.clone();
        custom.custom_endpoint = Some("https://gateway.internal/llm/chat/".into());
        assert_eq!(custom.request_url(), "https://gateway.internal/llm/chat");
        assert_eq!(
            config.request_url(),
            "https://example.com/v1/chat/completions"
        );
    }

    #[test]
    fn format_strings_are_stable() {
        assert_eq!(EndpointFormat::OpenAiChat.as_str(), "openai_chat");
        assert_eq!(
            EndpointFormat::AnthropicMessages.as_str(),
            "anthropic_messages"
        );
    }

    #[test]
    fn config_roundtrips_as_json() {
        let config = ProviderConfig::new("deepseek", "deepseek-chat")
            .with_base_url("https://api.deepseek.com")
            .with_format(EndpointFormat::OpenAiResponses);
        let json = serde_json::to_string(&config).unwrap();
        let back: ProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn configured_headers_and_context_window_are_preserved() {
        let config = ProviderConfig::new("test", "model")
            .with_headers(BTreeMap::from([("X-Tenant".into(), "ark".into())]));
        assert_eq!(config.headers.get("X-Tenant"), Some(&"ark".to_string()));
        assert_eq!(config.context_window_tokens, None);
    }
}
