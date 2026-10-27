//! Request preparation: guards, identity headers, credential injection.
//!
//! Split from [`crate::service::outbound`] so every decision here is a pure
//! function of (policy, plugin registration, request) and testable without a
//! socket. Errors carry the denial reason in `detail` for the audit sink.

use crate::config::HttpPolicy;
use crate::domain::allowlist::HostAllowlist;
use crate::domain::credential_ref::parse_reference;
use crate::domain::diagnostics::HttpDenialReason;
use crate::domain::request_guard::{
    host_of, parse_target, sanitize_headers, user_agent, PLUGIN_HEADER,
};
use crate::domain::trace::{accept_or_mint, TRACE_HEADER};
use crate::service::vault::CredentialVault;
use ark_common::{ArkError, Result};
use ark_ports::{HeaderMap, OutboundRequest};
use url::Url;

/// A request that passed every guard and is ready to be sent.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub plugin_id: String,
    pub version: String,
    pub method: String,
    pub url: Url,
    pub host: String,
    pub headers: HeaderMap,
    pub body: Option<String>,
    pub timeout_ms: u64,
    pub trace_id: String,
    pub credential_label: Option<String>,
    /// Raw credential reference, kept so a same-host redirect hop can re-inject
    /// it. Never contains the secret itself.
    pub credential_ref: Option<String>,
    /// Whether the request may be replayed without duplicating side effects.
    pub replayable: bool,
}

fn failure(reason: HttpDenialReason, message: impl Into<String>) -> ArkError {
    ArkError::validation(message).with_detail(reason.as_str())
}

/// Turn a denial detail back into the audit reason.
pub fn denial_reason(err: &ArkError) -> Option<HttpDenialReason> {
    let detail = err.detail.as_deref()?;
    Some(match detail {
        "host_not_allowed" => HttpDenialReason::HostNotAllowed,
        "empty_allowlist" => HttpDenialReason::EmptyAllowlist,
        "insecure_scheme" => HttpDenialReason::InsecureScheme,
        "unsupported_scheme" => HttpDenialReason::UnsupportedScheme,
        "invalid_url" => HttpDenialReason::InvalidUrl,
        "reserved_header" => HttpDenialReason::ReservedHeader,
        "rate_limited" => HttpDenialReason::RateLimited,
        "credential_denied" => HttpDenialReason::CredentialDenied,
        "body_too_large" => HttpDenialReason::BodyTooLarge,
        _ => return None,
    })
}

/// Validate the URL against policy and the manifest allowlist.
fn guard_target(allowlist: &HostAllowlist, policy: &HttpPolicy, raw: &str) -> Result<Url> {
    let url = parse_target(raw).map_err(|err| {
        let reason = if err.detail.as_deref() == Some("unsupported_scheme") {
            HttpDenialReason::UnsupportedScheme
        } else {
            HttpDenialReason::InvalidUrl
        };
        failure(reason, err.message)
    })?;
    if url.scheme() == "http" && !policy.allow_insecure_http {
        return Err(failure(
            HttpDenialReason::InsecureScheme,
            "plain http is disabled by the capability bus",
        ));
    }
    if allowlist.is_empty() {
        return Err(failure(
            HttpDenialReason::EmptyAllowlist,
            "plugin declares no http allowlist, egress denied",
        ));
    }
    if !allowlist.permits(&host_of(&url)) {
        return Err(failure(
            HttpDenialReason::HostNotAllowed,
            format!("host {} is not in the plugin allowlist", host_of(&url)),
        ));
    }
    Ok(url)
}

/// Inject the credential referenced by name; the plugin never sees plaintext.
fn inject_credential(
    vault: &CredentialVault,
    plugin_id: &str,
    reference: &str,
    headers: &mut HeaderMap,
) -> Result<Option<String>> {
    let cred = parse_reference(reference)?;
    let (secret, label) = vault.resolve(plugin_id, &cred.name)?;
    headers.insert(cred.header.clone(), cred.header_value(&secret));
    Ok(Some(label))
}

/// Build the final request. Fails closed on every guard.
pub fn prepare(
    policy: &HttpPolicy,
    plugin_id: &str,
    version: &str,
    allowlist: &HostAllowlist,
    vault: &CredentialVault,
    req: &OutboundRequest,
) -> Result<Prepared> {
    let method = if req.method.trim().is_empty() {
        "GET".to_string()
    } else {
        req.method.trim().to_ascii_uppercase()
    };
    let url = guard_target(allowlist, policy, &req.url)?;
    let mut headers = sanitize_headers(&req.headers)?;
    let trace_id = accept_or_mint(&req.headers);
    headers.remove(TRACE_HEADER);
    if let Some(body) = req.body.as_ref() {
        if body.len() > policy.max_body_bytes {
            return Err(failure(
                HttpDenialReason::BodyTooLarge,
                format!(
                    "request body {} bytes exceeds cap {}",
                    body.len(),
                    policy.max_body_bytes
                ),
            ));
        }
    }
    let credential_label = match req.credential.as_deref() {
        Some(reference) if !reference.trim().is_empty() => {
            inject_credential(vault, plugin_id, reference, &mut headers)?
        }
        _ => None,
    };
    let replayable = req
        .idempotency_key
        .as_deref()
        .map(|key| !key.trim().is_empty())
        .unwrap_or(false)
        || crate::domain::retry::is_idempotent(&method);
    if let Some(key) = req.idempotency_key.as_deref() {
        if !key.trim().is_empty() {
            headers.insert("idempotency-key".to_string(), key.trim().to_string());
        }
    }
    headers.insert("user-agent".to_string(), user_agent(plugin_id, version));
    headers.insert(PLUGIN_HEADER.to_string(), plugin_id.to_string());
    headers.insert(TRACE_HEADER.to_string(), trace_id.clone());
    Ok(Prepared {
        plugin_id: plugin_id.to_string(),
        version: version.to_string(),
        method,
        host: host_of(&url),
        url,
        headers,
        body: req.body.clone(),
        timeout_ms: policy.timeout_ms(req.timeout_ms),
        trace_id,
        credential_label,
        credential_ref: req.credential.clone().filter(|r| !r.trim().is_empty()),
        replayable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::diagnostics::MemoryDiagnostics;
    use std::sync::Arc;

    fn allowlist(entries: &[&str]) -> HostAllowlist {
        HostAllowlist::parse(&entries.iter().map(|e| e.to_string()).collect::<Vec<_>>()).unwrap()
    }

    fn req(url: &str) -> OutboundRequest {
        OutboundRequest {
            method: "GET".into(),
            url: url.into(),
            ..OutboundRequest::default()
        }
    }

    fn prepare_default(
        policy: &HttpPolicy,
        allowlist: &HostAllowlist,
        request: &OutboundRequest,
    ) -> Result<Prepared> {
        let vault = CredentialVault::new();
        prepare(
            policy,
            "ark.example.jira",
            "1.2.3",
            allowlist,
            &vault,
            request,
        )
    }

    #[test]
    fn host_outside_allowlist_is_denied() {
        let policy = HttpPolicy::default();
        let allow = allowlist(&["api.example.com"]);
        let err = prepare_default(&policy, &allow, &req("https://api.evil.test/v1")).unwrap_err();
        assert_eq!(denial_reason(&err), Some(HttpDenialReason::HostNotAllowed));
    }

    #[test]
    fn empty_allowlist_denies_all_egress() {
        let policy = HttpPolicy::default();
        let allow = HostAllowlist::default();
        let err = prepare_default(&policy, &allow, &req("https://api.example.com/v1")).unwrap_err();
        assert_eq!(denial_reason(&err), Some(HttpDenialReason::EmptyAllowlist));
    }

    #[test]
    fn insecure_scheme_is_denied_by_default() {
        let policy = HttpPolicy::default();
        let allow = allowlist(&["127.0.0.1"]);
        let err = prepare_default(&policy, &allow, &req("http://127.0.0.1:8080/x")).unwrap_err();
        assert_eq!(denial_reason(&err), Some(HttpDenialReason::InsecureScheme));

        let permissive = HttpPolicy::loopback_permissive();
        assert!(prepare_default(&permissive, &allow, &req("http://127.0.0.1:8080/x")).is_ok());
    }

    #[test]
    fn user_agent_and_plugin_header_are_host_owned() {
        let allow = allowlist(&["api.example.com"]);
        let prepared = prepare_default(
            &HttpPolicy::default(),
            &allow,
            &req("https://api.example.com/v1"),
        )
        .unwrap();
        assert_eq!(
            prepared.headers["user-agent"],
            "ark-plugin/ark.example.jira@1.2.3"
        );
        assert_eq!(prepared.headers["x-plugin-id"], "ark.example.jira");
        assert!(prepared.headers.contains_key(TRACE_HEADER));
    }

    #[test]
    fn plugin_cannot_override_the_user_agent() {
        let allow = allowlist(&["api.example.com"]);
        let mut request = req("https://api.example.com/v1");
        request.headers.insert("user-agent".into(), "evil/9".into());
        let err = prepare_default(&HttpPolicy::default(), &allow, &request).unwrap_err();
        assert_eq!(denial_reason(&err), Some(HttpDenialReason::ReservedHeader));
    }

    #[test]
    fn credential_is_injected_and_only_the_label_is_returned() {
        let allow = allowlist(&["api.example.com"]);
        let vault = CredentialVault::new();
        vault
            .store_for("ark.example.jira", "token", "s3cret-value")
            .unwrap();
        let mut request = req("https://api.example.com/v1");
        request.credential = Some("token".into());
        let prepared = prepare(
            &HttpPolicy::default(),
            "ark.example.jira",
            "1.2.3",
            &allow,
            &vault,
            &request,
        )
        .unwrap();
        assert_eq!(prepared.headers["authorization"], "Bearer s3cret-value");
        let label = prepared.credential_label.unwrap();
        assert!(label.starts_with("token@"));
        assert!(!label.contains("s3cret-value"));
    }

    #[test]
    fn credential_of_another_plugin_is_denied() {
        let allow = allowlist(&["api.example.com"]);
        let vault = CredentialVault::new();
        vault
            .store_for("ark.example.jira", "token", "s3cret")
            .unwrap();
        let mut request = req("https://api.example.com/v1");
        request.credential = Some("token".into());
        let err = prepare(
            &HttpPolicy::default(),
            "ark.example.evil",
            "0.1.0",
            &allow,
            &vault,
            &request,
        )
        .unwrap_err();
        assert_eq!(
            denial_reason(&err),
            Some(HttpDenialReason::CredentialDenied)
        );
    }

    #[test]
    fn oversize_body_is_denied_before_sending() {
        let policy = HttpPolicy {
            max_body_bytes: 8,
            ..HttpPolicy::default()
        };
        let allow = allowlist(&["api.example.com"]);
        let mut request = req("https://api.example.com/v1");
        request.body = Some("0123456789".into());
        let err = prepare_default(&policy, &allow, &request).unwrap_err();
        assert_eq!(denial_reason(&err), Some(HttpDenialReason::BodyTooLarge));
    }

    #[test]
    fn trace_id_is_propagated_when_valid() {
        let allow = allowlist(&["api.example.com"]);
        let mut request = req("https://api.example.com/v1");
        request
            .headers
            .insert(TRACE_HEADER.into(), "tr-upstream-1".into());
        let prepared = prepare_default(&HttpPolicy::default(), &allow, &request).unwrap();
        assert_eq!(prepared.trace_id, "tr-upstream-1");
    }

    #[test]
    fn sink_type_is_usable_as_a_trait_object() {
        let sink: Arc<dyn crate::domain::HttpDiagnosticSink> = Arc::new(MemoryDiagnostics::new(4));
        assert!(Arc::strong_count(&sink) == 1);
    }
}
