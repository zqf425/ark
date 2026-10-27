//! One physical send (design 7.5.1). Owns the single `reqwest::Client`.
//!
//! Plugins never build a client; this is the only place transport is
//! configured, so proxy/TLS/timeout policy cannot drift per plugin.

use crate::config::HttpPolicy;
use crate::domain::diagnostics::HttpDenialReason;
use crate::domain::redirect::{next_hop, redirect_budget, Hop};
use crate::service::prepare::{prepare, Prepared};
use crate::service::vault::CredentialVault;
use ark_common::{ArkError, ArkErrorCode, Result};
use ark_ports::{HeaderMap, OutboundRequest, OutboundResponse};
use url::Url;

/// Transport-level failure kind, so the retry decision is explicit.
#[derive(Debug)]
pub enum ExecuteError {
    /// Connection refused/reset/timeout: retryable for safe methods.
    Transport(String),
    /// Denied by a guard (allowlist, credential, size): never retried.
    Denied {
        reason: HttpDenialReason,
        message: String,
    },
}

impl ExecuteError {
    pub fn retryable(&self) -> bool {
        matches!(self, ExecuteError::Transport(_))
    }
}

impl From<ExecuteError> for ArkError {
    fn from(err: ExecuteError) -> Self {
        match err {
            ExecuteError::Transport(message) => {
                ArkError::new(ArkErrorCode::RuntimeUnavailable, message)
            }
            ExecuteError::Denied { reason, message } => {
                ArkError::validation(message).with_detail(reason.as_str())
            }
        }
    }
}

/// Wrapper around the shared client with bounded body reads.
pub struct Executor {
    client: reqwest::Client,
    policy: HttpPolicy,
}

impl Executor {
    /// Build the one client for the bus.
    pub fn new(policy: HttpPolicy) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_millis(policy.max_timeout_ms))
            .danger_accept_invalid_certs(policy.accept_invalid_certs)
            .pool_max_idle_per_host(8)
            .user_agent("ark-plugin/bus");
        if let Some(proxy) = policy.proxy.as_deref().filter(|p| !p.trim().is_empty()) {
            builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|err| {
                ArkError::validation(format!("invalid proxy: {err}")).with_detail("invalid_url")
            })?);
        }
        let client = builder
            .build()
            .map_err(|err| ArkError::internal(format!("failed to build http client: {err}")))?;
        Ok(Self { client, policy })
    }

    pub fn policy(&self) -> &HttpPolicy {
        &self.policy
    }

    /// Send exactly one hop. No retry, no redirect following.
    pub async fn send(
        &self,
        method: &str,
        url: &Url,
        headers: &HeaderMap,
        body: Option<&str>,
        timeout_ms: u64,
    ) -> std::result::Result<OutboundResponse, ExecuteError> {
        let mut builder = self
            .client
            .request(method.parse().unwrap_or(reqwest::Method::GET), url.clone())
            .timeout(std::time::Duration::from_millis(timeout_ms));
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = body {
            builder = builder.body(body.to_string());
        }
        let response = builder.send().await.map_err(|err| {
            if err.is_timeout() || err.is_connect() || err.is_request() {
                ExecuteError::Transport(format!("{method} {url}: {err}"))
            } else {
                ExecuteError::Transport(format!("{method} {url}: {err}"))
            }
        })?;
        let status = response.status().as_u16();
        let headers = self.collect_headers(response.headers());
        let body = self.read_body(response).await?;
        Ok(OutboundResponse {
            status,
            headers,
            body,
        })
    }

    fn collect_headers(&self, source: &reqwest::header::HeaderMap) -> HeaderMap {
        source
            .iter()
            .take(self.policy.max_headers)
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
            })
            .collect()
    }

    /// Read at most `max_body_bytes`; exceeding the cap is a denial, not a
    /// silent truncation, so a plugin can never act on a partial payload.
    async fn read_body(
        &self,
        mut response: reqwest::Response,
    ) -> std::result::Result<String, ExecuteError> {
        let cap = self.policy.max_body_bytes;
        if response.content_length().unwrap_or(0) as usize > cap {
            return Err(ExecuteError::Denied {
                reason: HttpDenialReason::BodyTooLarge,
                message: format!("response body exceeds cap of {cap} bytes"),
            });
        }
        let mut buffer: Vec<u8> = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if buffer.len() + chunk.len() > cap {
                        return Err(ExecuteError::Denied {
                            reason: HttpDenialReason::BodyTooLarge,
                            message: format!("response body exceeds cap of {cap} bytes"),
                        });
                    }
                    buffer.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(err) => {
                    return Err(ExecuteError::Transport(format!("body read failed: {err}")))
                }
            }
        }
        Ok(String::from_utf8_lossy(&buffer).into_owned())
    }

    /// Rebuild the request for a redirect hop, re-running the allowlist guard.
    ///
    /// Credentials are re-injected only when the hop stays on the same host: an
    /// allowlisted redirect to a third party must never carry the secret.
    pub fn prepare_hop(
        &self,
        previous: &Prepared,
        target: &Url,
        vault: &CredentialVault,
        allowlist: &crate::domain::HostAllowlist,
    ) -> std::result::Result<Prepared, ExecuteError> {
        let same_host = crate::domain::host_of(target) == previous.host;
        let request = OutboundRequest {
            method: previous.method.clone(),
            url: target.to_string(),
            headers: previous
                .headers
                .iter()
                .filter(|(name, _)| {
                    let lower = name.to_ascii_lowercase();
                    !matches!(
                        lower.as_str(),
                        "user-agent" | "x-plugin-id" | "authorization" | "idempotency-key"
                    ) && !crate::domain::is_reserved_header(&lower)
                })
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            body: previous.body.clone(),
            timeout_ms: Some(previous.timeout_ms),
            idempotency_key: None,
            credential: previous.credential_ref.clone().filter(|_| same_host),
        };
        prepare(
            &self.policy,
            &previous.plugin_id,
            &previous.version,
            allowlist,
            vault,
            &request,
        )
        .map_err(|err| ExecuteError::Denied {
            reason: crate::service::prepare::denial_reason(&err)
                .unwrap_or(HttpDenialReason::HostNotAllowed),
            message: err.message,
        })
    }

    /// Follow one redirect when policy allows it.
    pub fn hop_for(
        &self,
        current: &Url,
        status: u16,
        location: Option<&str>,
        followed: u8,
    ) -> std::result::Result<Option<(Url, bool)>, ExecuteError> {
        if followed >= redirect_budget(&self.policy) {
            return Ok(None);
        }
        match next_hop(current, status, location).map_err(|err| ExecuteError::Denied {
            reason: HttpDenialReason::InvalidUrl,
            message: err.message,
        })? {
            Hop::Follow { target, drop_body } => Ok(Some((target, drop_body))),
            Hop::Stop => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_budget_zero_when_disabled() {
        let mut policy = HttpPolicy::default();
        policy.follow_redirects = false;
        let executor = Executor::new(policy).unwrap();
        let url = Url::parse("https://api.example.com/a").unwrap();
        assert!(executor
            .hop_for(&url, 302, Some("/b"), 0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn redirect_hop_respects_max_redirects() {
        let policy = HttpPolicy {
            max_redirects: 1,
            ..HttpPolicy::default()
        };
        let executor = Executor::new(policy).unwrap();
        let url = Url::parse("https://api.example.com/a").unwrap();
        assert!(executor
            .hop_for(&url, 302, Some("/b"), 0)
            .unwrap()
            .is_some());
        assert!(executor
            .hop_for(&url, 302, Some("/b"), 1)
            .unwrap()
            .is_none());
    }

    #[test]
    fn transport_errors_are_retryable_and_denials_are_not() {
        assert!(ExecuteError::Transport("reset".into()).retryable());
        assert!(!ExecuteError::Denied {
            reason: HttpDenialReason::HostNotAllowed,
            message: "x".into()
        }
        .retryable());
    }
}
