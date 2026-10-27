//! The HTTP capability implementation (design 7.5.1).
//!
//! One shared client, one allowlist decision per request *and* per redirect
//! hop, one rate-limit dimension (`plugin_id + host`) and one audit sink.
//! Plugins only ever see [`ark_ports::HttpCapability`].

use crate::config::HttpPolicy;
use crate::domain::diagnostics::{HttpDenialReason, HttpDiagnostic, HttpDiagnosticSink};
use crate::domain::rate_limit::RateLimiter;
use crate::domain::request_guard::redact_url;
use crate::domain::retry::{backoff_ms, is_idempotent, retryable_status, should_retry};
use crate::service::egress::{EgressGrant, EgressRegistry};
use crate::service::execute::{ExecuteError, Executor};
use crate::service::prepare::{denial_reason, prepare, Prepared};
use crate::service::vault::CredentialVault;
use ark_common::{ArkError, Result};
use ark_ports::{HttpCapability, OutboundRequest, OutboundResponse};
use async_trait::async_trait;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

/// Builder keeps construction explicit: sink and vault are host-supplied.
pub struct HttpBusBuilder {
    policy: HttpPolicy,
    vault: Arc<CredentialVault>,
    sink: Arc<dyn HttpDiagnosticSink>,
}

impl HttpBusBuilder {
    pub fn new(sink: Arc<dyn HttpDiagnosticSink>) -> Self {
        Self {
            policy: HttpPolicy::default(),
            vault: Arc::new(CredentialVault::new()),
            sink,
        }
    }

    pub fn policy(mut self, policy: HttpPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn vault(mut self, vault: Arc<CredentialVault>) -> Self {
        self.vault = vault;
        self
    }

    pub fn build(self) -> Result<HttpBus> {
        HttpBus::new(self.policy, self.vault, self.sink)
    }
}

/// One finished logical request, kept for span reporting.
struct Completed {
    response: OutboundResponse,
    prepared: Prepared,
    attempts: u32,
}

/// The single egress for every plugin.
pub struct HttpBus {
    policy: HttpPolicy,
    executor: Executor,
    registry: RwLock<EgressRegistry>,
    vault: Arc<CredentialVault>,
    limiter: Mutex<RateLimiter>,
    sink: Arc<dyn HttpDiagnosticSink>,
}

impl HttpBus {
    pub fn new(
        policy: HttpPolicy,
        vault: Arc<CredentialVault>,
        sink: Arc<dyn HttpDiagnosticSink>,
    ) -> Result<Self> {
        let executor = Executor::new(policy.clone())?;
        let limiter = RateLimiter::new(policy.rate.clone());
        Ok(Self {
            policy,
            executor,
            registry: RwLock::new(EgressRegistry::new()),
            vault,
            limiter: Mutex::new(limiter),
            sink,
        })
    }

    pub fn builder(sink: Arc<dyn HttpDiagnosticSink>) -> HttpBusBuilder {
        HttpBusBuilder::new(sink)
    }

    pub fn policy(&self) -> &HttpPolicy {
        &self.policy
    }

    pub fn vault(&self) -> &Arc<CredentialVault> {
        &self.vault
    }

    /// Declare a plugin's egress rights. Called by the host at load time.
    pub fn register_plugin(&self, plugin_id: &str, version: &str, allow: &[String]) -> Result<()> {
        let mut registry = self
            .registry
            .write()
            .map_err(|_| ArkError::internal("egress registry poisoned"))?;
        registry.register(plugin_id, version, allow).map(|_| ())
    }

    pub fn unregister_plugin(&self, plugin_id: &str) -> bool {
        self.registry
            .write()
            .map(|mut registry| registry.unregister(plugin_id))
            .unwrap_or(false)
    }

    fn grant(&self, plugin_id: &str) -> Result<EgressGrant> {
        let registry = self
            .registry
            .read()
            .map_err(|_| ArkError::internal("egress registry poisoned"))?;
        registry.require(plugin_id).cloned()
    }

    /// Consume one token for `plugin_id + host`; `false` means over quota.
    fn acquire_quota(&self, plugin_id: &str, host: &str) -> bool {
        match self.limiter.lock() {
            Ok(mut limiter) => limiter.acquire(plugin_id, host, Instant::now()),
            // A poisoned limiter must not become an open gate.
            Err(_) => false,
        }
    }

    fn record_denial(
        &self,
        plugin_id: &str,
        method: &str,
        raw_url: &str,
        reason: HttpDenialReason,
        detail: &str,
    ) {
        let redacted = redact_for_log(raw_url);
        self.sink.record(
            HttpDiagnostic::denied(plugin_id, method, &redacted, reason, detail)
                .with_host(host_for_log(raw_url).as_str()),
        );
    }

    fn record_span(&self, completed: &Completed, elapsed_ms: u64) {
        self.sink.record(HttpDiagnostic {
            kind: "http_span".to_string(),
            plugin_id: completed.prepared.plugin_id.clone(),
            host: completed.prepared.host.clone(),
            method: completed.prepared.method.clone(),
            redacted_url: redact_url(&completed.prepared.url),
            reason: None,
            detail: completed.prepared.credential_label.clone(),
            status: Some(completed.response.status),
            duration_ms: Some(elapsed_ms),
            attempts: Some(completed.attempts),
        });
    }

    /// Follow redirects for one attempt, re-guarding every hop.
    async fn deliver_hops(
        &self,
        prepared: &mut Prepared,
        grant: &EgressGrant,
    ) -> std::result::Result<OutboundResponse, ExecuteError> {
        let mut followed: u8 = 0;
        loop {
            let response = self
                .executor
                .send(
                    &prepared.method,
                    &prepared.url,
                    &prepared.headers,
                    prepared.body.as_deref(),
                    prepared.timeout_ms,
                )
                .await?;
            let location = response.headers.get("location").cloned();
            let hop = self.executor.hop_for(
                &prepared.url,
                response.status,
                location.as_deref(),
                followed,
            )?;
            let Some((target, drop_body)) = hop else {
                return Ok(response);
            };
            followed += 1;
            let next =
                self.executor
                    .prepare_hop(prepared, &target, &self.vault, &grant.allowlist)?;
            *prepared = next;
            if drop_body {
                prepared.method = "GET".to_string();
                prepared.body = None;
            }
        }
    }

    /// One logical request with retry/backoff on transport errors and 5xx.
    async fn deliver(
        &self,
        plugin_id: &str,
        req: &OutboundRequest,
    ) -> std::result::Result<Completed, ExecuteError> {
        let grant = self.grant(plugin_id).map_err(|err| ExecuteError::Denied {
            reason: denial_reason(&err).unwrap_or(HttpDenialReason::EmptyAllowlist),
            message: err.message,
        })?;
        let prepared = prepare(
            &self.policy,
            plugin_id,
            &grant.version,
            &grant.allowlist,
            &self.vault,
            req,
        )
        .map_err(|err| ExecuteError::Denied {
            reason: denial_reason(&err).unwrap_or(HttpDenialReason::InvalidUrl),
            message: err.message,
        })?;
        if !self.acquire_quota(plugin_id, &prepared.host) {
            return Err(ExecuteError::Denied {
                reason: HttpDenialReason::RateLimited,
                message: format!("rate limit reached for {plugin_id} on {}", prepared.host),
            });
        }
        let replayable = prepared.replayable || is_idempotent(&prepared.method);
        let mut attempt: u32 = 1;
        loop {
            match self.deliver_hops(&mut prepared.clone(), &grant).await {
                Ok(response) => {
                    if retryable_status(response.status)
                        && should_retry(
                            &prepared.method,
                            Some(response.status),
                            replayable,
                            attempt,
                            &self.policy.retry,
                        )
                    {
                        attempt += 1;
                        sleep_ms(backoff_ms(attempt - 1, &self.policy.retry)).await;
                        continue;
                    }
                    return Ok(Completed {
                        response,
                        prepared,
                        attempts: attempt,
                    });
                }
                Err(err) => {
                    let retry = err.retryable()
                        && should_retry(
                            &prepared.method,
                            None,
                            replayable,
                            attempt,
                            &self.policy.retry,
                        );
                    if !retry {
                        return Err(err);
                    }
                    attempt += 1;
                    sleep_ms(backoff_ms(attempt - 1, &self.policy.retry)).await;
                }
            }
        }
    }

    /// Instrumented entry point used by [`HttpCapability::request`].
    async fn execute(&self, plugin_id: &str, req: OutboundRequest) -> Result<OutboundResponse> {
        let method = if req.method.trim().is_empty() {
            "GET".to_string()
        } else {
            req.method.trim().to_ascii_uppercase()
        };
        let started = Instant::now();
        match self.deliver(plugin_id, &req).await {
            Ok(completed) => {
                let elapsed = started.elapsed().as_millis() as u64;
                self.record_span(&completed, elapsed);
                Ok(completed.response)
            }
            Err(err) => {
                if let ExecuteError::Denied { reason, message } = &err {
                    self.record_denial(plugin_id, &method, &req.url, *reason, message);
                }
                Err(err.into())
            }
        }
    }
}

#[async_trait]
impl HttpCapability for HttpBus {
    async fn request(&self, plugin_id: &str, req: OutboundRequest) -> Result<OutboundResponse> {
        self.execute(plugin_id, req).await
    }
}

fn redact_for_log(raw: &str) -> String {
    match crate::domain::parse_target(raw) {
        Ok(url) => redact_url(&url),
        Err(_) => "<invalid-url>".to_string(),
    }
}

fn host_for_log(raw: &str) -> String {
    crate::domain::parse_target(raw)
        .map(|url| crate::domain::host_of(&url))
        .unwrap_or_default()
}

async fn sleep_ms(ms: u64) {
    if ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
}

#[cfg(test)]
mod tests;
