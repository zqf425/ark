//! Provider model-catalog protocol parsing.

use ark_common::{ArkError, Result};
use ark_contract::{ProviderModelCatalogResponse, ProviderModelSummary};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

use crate::provider::route::Family;
use crate::ProviderConfig;

/// Maximum models returned to the settings UI from one remote response.
///
/// Large aggregators can expose thousands of entries. A settings picker needs a
/// bounded, deterministic subset instead of retaining an unbounded response in
/// both the server and browser.
pub const MAX_CATALOG_MODELS: usize = 500;

/// Maximum response bytes accepted from a remote model catalog.
pub const MAX_CATALOG_BODY_BYTES: usize = 1024 * 1024;

/// Wall-clock budget for an operator-triggered model discovery request.
pub const CATALOG_TIMEOUT: Duration = Duration::from_secs(10);

/// Reusable, bounded HTTP client for provider model discovery.
///
/// It only runs on an explicit settings-page action; it neither polls providers
/// nor caches secrets. `reqwest::Client` owns its connection pool, so repeated
/// discovery calls reuse connections instead of allocating a new pool.
#[derive(Clone)]
pub struct ModelCatalogClient {
    client: reqwest::Client,
    timeout: Duration,
}

impl Default for ModelCatalogClient {
    fn default() -> Self {
        Self::new(reqwest::Client::new())
    }
}

impl ModelCatalogClient {
    /// Create a client around the process-level HTTP client.
    ///
    /// Parameter: `client` carries the caller's proxy, TLS and pool policy.
    /// Returns: a catalog client with the standard discovery timeout.
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            timeout: CATALOG_TIMEOUT,
        }
    }

    /// Override the bounded discovery timeout, primarily for deterministic tests.
    ///
    /// Parameter: `timeout` is the total request budget.
    /// Returns: the updated catalog client.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Query and normalize a provider model catalog.
    ///
    /// Parameter: `config` supplies the selected API family, credential and
    /// custom headers. Returns a bounded, sorted catalog or a scrubbed error.
    pub async fn list(&self, config: &ProviderConfig) -> Result<ProviderModelCatalogResponse> {
        let mut request = self
            .client
            .get(model_catalog_url(config))
            .timeout(self.timeout);
        for (name, value) in Family::of(config).headers(config) {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|_| catalog_transport_failure(&config.provider_id))?;
        let status = response.status();
        if !status.is_success() {
            // Carry the upstream status so the settings UI can tell an auth
            // failure (401/403) from a wrong endpoint (404) instead of showing
            // one generic "model request failed" for every case.
            return Err(ArkError::model(format!(
                "model catalog request for provider `{}` returned HTTP {}",
                config.provider_id,
                status.as_u16()
            ))
            .with_upstream_status(status.as_u16()));
        }
        if response.content_length().is_some_and(|size| {
            usize::try_from(size).map_or(true, |size| size > MAX_CATALOG_BODY_BYTES)
        }) {
            return Err(ArkError::model(format!(
                "model catalog response for provider `{}` exceeds the {} byte limit",
                config.provider_id, MAX_CATALOG_BODY_BYTES
            )));
        }
        let mut bytes = Vec::new();
        let mut response = response;
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            ArkError::model(format!(
                "model catalog response for provider `{}` could not be read: {error}",
                config.provider_id
            ))
        })? {
            let remaining = MAX_CATALOG_BODY_BYTES.saturating_sub(bytes.len());
            if chunk.len() > remaining {
                return Err(ArkError::model(format!(
                    "model catalog response for provider `{}` exceeds the {} byte limit",
                    config.provider_id, MAX_CATALOG_BODY_BYTES
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = std::str::from_utf8(&bytes)
            .map_err(|_| ArkError::model("model catalog response is not UTF-8 JSON"))?;
        parse_model_catalog_response(body)
    }
}

/// Keep transport errors free of `reqwest`'s URL text, because a custom base
/// URL can contain credential query parameters.
fn catalog_transport_failure(provider_id: &str) -> ArkError {
    ArkError::model(format!(
        "model catalog request failed for provider `{provider_id}`; check the base URL and network"
    ))
}

/// Resolve the standard `{base}/models` endpoint from a provider base URL.
///
/// The version-segment rule lives in [`crate::config::url::model_catalog_url`],
/// shared with the chat families: discovery must hit the same version prefix a
/// turn would, otherwise a provider that answers `/v4/chat/completions` also
/// answers 404 for `/v4/v1/models`. This wrapper only supplies the resolved base
/// URL and never exposes the URL in an error message.
fn model_catalog_url(config: &ProviderConfig) -> String {
    crate::config::url::model_catalog_url(&config.resolved_base_url())
}

/// Parse common provider catalog shapes into the stable wire response.
///
/// The parser deliberately records only explicit metadata. A model identifier
/// does not imply a context length or multimodal support, so missing fields stay
/// `None` rather than being guessed from a name prefix.
pub(crate) fn parse_model_catalog_response(body: &str) -> Result<ProviderModelCatalogResponse> {
    let root: Value = serde_json::from_str(body).map_err(|error| {
        ArkError::model(format!("model catalog returned invalid JSON: {error}"))
    })?;
    let values = root
        .get("data")
        .or_else(|| root.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(|| ArkError::model("model catalog response has no `data` or `models` array"))?;

    let mut by_id = BTreeMap::new();
    for value in values {
        let Some(id) = string_at(value, &["id"]).or_else(|| string_at(value, &["slug"])) else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        by_id
            .entry(id.to_string())
            .or_insert_with(|| ProviderModelSummary {
                id: id.to_string(),
                label: string_at(value, &["name"]).map(str::to_string),
                context_window_tokens: number_at(
                    value,
                    &[
                        &["context_length"],
                        &["context_window"],
                        &["context_window_tokens"],
                        &["max_context_length"],
                        &["limits", "context_length"],
                    ],
                ),
                supports_vision: vision_at(value),
                max_output_tokens: number_at(
                    value,
                    &[
                        &["max_completion_tokens"],
                        &["max_output_tokens"],
                        &["max_tokens"],
                        &["top_provider", "max_completion_tokens"],
                    ],
                )
                .and_then(|value| u32::try_from(value).ok()),
            });
    }

    Ok(ProviderModelCatalogResponse {
        models: by_id.into_values().take(MAX_CATALOG_MODELS).collect(),
    })
}

fn string_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str()
}

fn number_at(value: &Value, paths: &[&[&str]]) -> Option<u64> {
    paths.iter().find_map(|path| {
        let mut current = value;
        for key in *path {
            current = current.get(*key)?;
        }
        current
            .as_u64()
            .or_else(|| current.as_str()?.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
    })
}

fn vision_at(value: &Value) -> Option<bool> {
    bool_at(value, &[&["supports_vision"], &["capabilities", "vision"]]).or_else(|| {
        modalities_at(value, &["architecture", "input_modalities"])
            .or_else(|| modalities_at(value, &["input_modalities"]))
            .map(|modalities| {
                modalities
                    .iter()
                    .any(|modality| modality.eq_ignore_ascii_case("image"))
            })
    })
}

fn bool_at(value: &Value, paths: &[&[&str]]) -> Option<bool> {
    paths.iter().find_map(|path| {
        let mut current = value;
        for key in *path {
            current = current.get(*key)?;
        }
        current.as_bool()
    })
}

fn modalities_at<'a>(value: &'a Value, path: &[&str]) -> Option<Vec<&'a str>> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    let array = current.as_array()?;
    Some(array.iter().filter_map(Value::as_str).collect())
}

#[cfg(test)]
mod tests {
    use super::{catalog_transport_failure, model_catalog_url, parse_model_catalog_response};
    use crate::{EndpointFormat, ProviderConfig};

    #[test]
    fn rich_catalog_metadata_is_preserved_without_guessing() {
        let catalog = parse_model_catalog_response(
            r#"{
                "data": [{
                    "id": "openai/gpt-4o",
                    "name": "GPT-4o",
                    "context_length": 128000,
                    "architecture": {"input_modalities": ["text", "image"]},
                    "top_provider": {"max_completion_tokens": 16384}
                }, {
                    "id": "text-only",
                    "architecture": {"input_modalities": ["text"]}
                }, {
                    "id": "unknown-shape"
                }]
            }"#,
        )
        .unwrap();

        assert_eq!(catalog.models.len(), 3);
        assert_eq!(catalog.models[0].id, "openai/gpt-4o");
        assert_eq!(catalog.models[0].label.as_deref(), Some("GPT-4o"));
        assert_eq!(catalog.models[0].context_window_tokens, Some(128_000));
        assert_eq!(catalog.models[0].supports_vision, Some(true));
        assert_eq!(catalog.models[0].max_output_tokens, Some(16_384));
        assert_eq!(catalog.models[1].supports_vision, Some(false));
        assert_eq!(catalog.models[2].context_window_tokens, None);
        assert_eq!(catalog.models[2].supports_vision, None);
    }

    #[test]
    fn catalog_url_reuses_an_existing_version_prefix() {
        let v1 = ProviderConfig::new("catalog", "unused")
            .with_base_url("https://gateway.example/api/v1/");
        assert_eq!(
            model_catalog_url(&v1),
            "https://gateway.example/api/v1/models"
        );

        let root = ProviderConfig::new("catalog", "unused")
            .with_base_url("https://gateway.example")
            .with_format(EndpointFormat::OpenAiResponses);
        assert_eq!(
            model_catalog_url(&root),
            "https://gateway.example/v1/models"
        );
    }

    #[test]
    fn transport_failures_do_not_echo_a_base_url_or_secret() {
        let error = catalog_transport_failure("catalog");
        assert!(error.message.contains("catalog"));
        assert!(!error.message.contains("api_key"));
        assert!(!error.message.contains("secret"));
    }

    /// A non-2xx catalog answer must carry the upstream status, so the settings
    /// UI can tell a bad key (401) from a wrong base URL (404) instead of
    /// showing the same generic message for both.
    #[tokio::test]
    async fn a_failed_catalog_keeps_the_upstream_status() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4_096];
            let _ = socket.read(&mut request).unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });

        let config = ProviderConfig::new("catalog", "unused")
            .with_base_url(format!("http://{address}/v1"))
            .with_api_key("secret");
        let error = super::ModelCatalogClient::default()
            .list(&config)
            .await
            .unwrap_err();

        assert_eq!(error.upstream_status, Some(401));
        assert!(error.message.contains("401"), "{}", error.message);
        // The transport layer's own status stays the documented 502 for a
        // proxied model failure; only `upstream_status` names the provider.
        assert_eq!(
            error.code,
            ark_common::ArkErrorCode::ModelRequestFailed,
            "{}",
            error.message
        );
        server.join().unwrap();
    }
}
