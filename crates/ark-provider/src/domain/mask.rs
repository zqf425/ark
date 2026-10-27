//! Secret masking for read paths, logs and probe messages.
//!
//! Every value that leaves [`crate::ProviderStore`] for a caller passes through
//! this module first: the table view reports only `has_api_key` plus a hint, and
//! a URL or error string that may embed a credential is redacted before it is
//! returned or traced.

use crate::config::{KEY_HINT_MASK, KEY_HINT_MIN_LEN, KEY_HINT_PREFIX, KEY_HINT_SUFFIX};

/// Query parameter names whose value is a credential.
const SECRET_QUERY_NAMES: [&str; 8] = [
    "key",
    "api_key",
    "apikey",
    "token",
    "access_token",
    "password",
    "sig",
    "signature",
];

/// Mask one secret: `sk-1234567890abcdef` -> `sk-1***ef`.
///
/// The rule is prefix + suffix with a fixed mask in between. Anything shorter
/// than [`KEY_HINT_MIN_LEN`] is masked completely, because a 4+2 window would
/// reveal most of it.
pub fn mask_api_key(raw: &str) -> String {
    let trimmed = raw.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() < KEY_HINT_MIN_LEN {
        return KEY_HINT_MASK.to_string();
    }
    let head: String = chars.iter().take(KEY_HINT_PREFIX).collect();
    let tail: String = chars.iter().skip(chars.len() - KEY_HINT_SUFFIX).collect();
    format!("{head}{KEY_HINT_MASK}{tail}")
}

/// Hint for a view, or `None` when no key is stored.
pub fn api_key_hint(api_key: Option<&str>) -> Option<String> {
    let raw = api_key?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(mask_api_key(raw))
}

/// True when a query parameter name looks like a credential.
fn is_secret_param(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.contains("key") || lower.contains("token") || lower.contains("password") {
        return true;
    }
    if lower.contains("secret") || lower.contains("auth") {
        return true;
    }
    SECRET_QUERY_NAMES.contains(&lower.as_str())
}

/// Redact credential query values, keeping the URL readable.
///
/// A custom base URL may carry `?api_key=...`; that must never reach the GUI, a
/// log line or an error message. Input that `url::Url` cannot parse is still
/// scrubbed by [`redact_query_text`]: an *invalid* URL is exactly the case that
/// ends up quoted inside a validation error, so leaving it untouched would leak
/// the secret at the worst moment.
pub fn redact_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let Ok(mut parsed) = url::Url::parse(trimmed) else {
        return redact_query_text(trimmed);
    };
    if parsed.query().is_none() {
        return parsed.to_string();
    }
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(name, value)| {
            let masked = if is_secret_param(&name) {
                KEY_HINT_MASK.to_string()
            } else {
                value.to_string()
            };
            (name.to_string(), masked)
        })
        .collect();
    {
        let mut query = parsed.query_pairs_mut();
        query.clear();
        for (name, value) in pairs {
            query.append_pair(&name, &value);
        }
    }
    parsed.to_string()
}

/// Replace a literal secret with the mask inside arbitrary text.
///
/// Error strings from the model client embed the request URL, and a provider
/// base URL may contain the key, so every outgoing message is scrubbed.
pub fn scrub_secret(text: &str, secret: &str) -> String {
    let needle = secret.trim();
    if needle.is_empty() || needle.len() < 4 {
        return text.to_string();
    }
    text.replace(needle, KEY_HINT_MASK)
}

/// Best-effort scrub of `?name=value&...` pairs in text that is not a parseable
/// URL.
///
/// Values of credential-looking names are replaced outright; everything else is
/// preserved so the message still says which URL was rejected.
pub fn redact_query_text(text: &str) -> String {
    let Some((head, query)) = text.split_once('?') else {
        return text.to_string();
    };
    // Keep a fragment tail out of the value being rewritten.
    let (pairs, fragment) = match query.split_once('#') {
        Some((pairs, fragment)) => (pairs, Some(fragment)),
        None => (query, None),
    };
    let rewritten: Vec<String> = pairs
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) if is_secret_param(name) && !value.is_empty() => {
                format!("{name}={KEY_HINT_MASK}")
            }
            _ => pair.to_string(),
        })
        .collect();
    let mut out = format!("{head}?{}", rewritten.join("&"));
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_keys_keep_only_the_edges() {
        assert_eq!(mask_api_key("sk-1234567890abcdef"), "sk-1***ef");
        assert_eq!(mask_api_key("  sk-1234567890abcdef  "), "sk-1***ef");
    }

    #[test]
    fn keys_shorter_than_the_minimum_are_fully_masked() {
        for short in ["", "a", "sk-1", "abcdefg"] {
            assert_eq!(mask_api_key(short), KEY_HINT_MASK);
        }
        // Exactly at the threshold a hint is allowed again.
        assert_eq!(mask_api_key("abcdefgh"), "abcd***gh");
    }

    #[test]
    fn a_short_key_never_leaks_its_tail() {
        let hint = mask_api_key("abc1234");
        assert!(!hint.contains("234"));
        assert_eq!(hint, KEY_HINT_MASK);
    }

    #[test]
    fn hint_is_none_without_a_key() {
        assert_eq!(api_key_hint(None), None);
        assert_eq!(api_key_hint(Some("   ")), None);
        assert_eq!(
            api_key_hint(Some("sk-1234567890abcdef")).as_deref(),
            Some("sk-1***ef")
        );
    }

    #[test]
    fn credential_query_values_are_redacted() {
        let url = "https://proxy.local/v1/chat/completions?api_key=secret123&stream=1";
        let redacted = redact_url(url);
        assert!(!redacted.contains("secret123"), "{redacted}");
        assert!(redacted.contains("stream=1"), "{redacted}");
    }

    #[test]
    fn a_url_without_query_is_unchanged() {
        let url = "https://api.deepseek.com/v1/chat/completions";
        assert_eq!(redact_url(url), url);
    }

    #[test]
    fn scrubbing_removes_the_literal_secret() {
        let text = "request to https://x.local/v1?api_key=sk-secret-value failed";
        let scrubbed = scrub_secret(text, "sk-secret-value");
        assert!(!scrubbed.contains("sk-secret-value"));
        assert!(scrubbed.contains(KEY_HINT_MASK));
    }
}
