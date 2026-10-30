//! Redirect walking (design 7.5.1: one redirect policy, allowlist re-checked).
//!
//! Reqwest's automatic redirect following is disabled on purpose: a 302 that
//! points at `169.254.169.254` must be denied exactly like a direct request,
//! which means every hop goes back through the allowlist guard.

use ark_common::{ArkError, Result};
use url::Url;

/// Outcome of interpreting a 3xx response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hop {
    /// Follow `target`. `drop_body` is true when the status forces `GET`.
    Follow { target: Url, drop_body: bool },
    /// Not a redirect, or following it is not permitted.
    Stop,
}

/// Resolve `location` against the current URL and decide the next hop.
///
/// `301`/`302`/`303` downgrade to `GET` without a body (browser behaviour);
/// `307`/`308` preserve method and body.
pub fn next_hop(current: &Url, status: u16, location: Option<&str>) -> Result<Hop> {
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return Ok(Hop::Stop);
    }
    let Some(location) = location.map(str::trim).filter(|l| !l.is_empty()) else {
        return Ok(Hop::Stop);
    };
    let target = current
        .join(location)
        .map_err(|err| ArkError::validation(format!("invalid redirect target: {err}")))?;
    let drop_body = matches!(status, 301..=303);
    Ok(Hop::Follow { target, drop_body })
}

/// Maximum redirects actually followed for one logical request.
pub fn redirect_budget(policy: &crate::config::HttpPolicy) -> u8 {
    if policy.follow_redirects {
        policy.max_redirects
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn relative_location_resolves_against_the_current_url() {
        let hop = next_hop(&url("https://api.example.com/v1/a/b"), 302, Some("/v2/c")).unwrap();
        assert_eq!(
            hop,
            Hop::Follow {
                target: url("https://api.example.com/v2/c"),
                drop_body: true
            }
        );
    }

    #[test]
    fn absolute_location_is_used_as_is() {
        let hop = next_hop(
            &url("https://api.example.com/v1"),
            308,
            Some("https://cdn.example.com/obj"),
        )
        .unwrap();
        assert_eq!(
            hop,
            Hop::Follow {
                target: url("https://cdn.example.com/obj"),
                drop_body: false
            }
        );
    }

    #[test]
    fn non_redirect_status_stops() {
        assert_eq!(
            next_hop(&url("https://api.example.com"), 200, Some("/x")).unwrap(),
            Hop::Stop
        );
        assert_eq!(
            next_hop(&url("https://api.example.com"), 304, Some("/x")).unwrap(),
            Hop::Stop
        );
    }

    #[test]
    fn missing_or_empty_location_stops() {
        assert_eq!(
            next_hop(&url("https://api.example.com"), 302, None).unwrap(),
            Hop::Stop
        );
        assert_eq!(
            next_hop(&url("https://api.example.com"), 302, Some("  ")).unwrap(),
            Hop::Stop
        );
    }

    #[test]
    fn disabled_policy_has_no_budget() {
        let mut policy = crate::config::HttpPolicy::default();
        assert_eq!(redirect_budget(&policy), policy.max_redirects);
        policy.follow_redirects = false;
        assert_eq!(redirect_budget(&policy), 0);
    }
}
