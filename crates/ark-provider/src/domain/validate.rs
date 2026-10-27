//! Validation rules for one provider entry.
//!
//! Every failure is an `ArkErrorCode::ValidationError` whose message is written
//! for the settings dialog: it names the offending field and the accepted
//! values, so the GUI can show it verbatim.

use ark_common::{ArkError, Result};
use ark_contract::ProviderEntry;
use ark_llm::EndpointFormat;

use crate::config::ID_MAX_LEN;

/// Validate and normalize the id: non-empty and `^[A-Za-z0-9._-]{1,64}$`.
pub fn validate_id(raw: &str) -> Result<String> {
    let id = raw.trim();
    if id.is_empty() {
        return Err(ArkError::validation(
            "provider id is required; use letters, digits, dot, dash or underscore",
        ));
    }
    if id.chars().count() > ID_MAX_LEN {
        return Err(ArkError::validation(format!(
            "provider id is too long: at most {ID_MAX_LEN} characters"
        )));
    }
    let valid = id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'));
    if !valid {
        return Err(ArkError::validation(format!(
            "provider id `{id}` contains unsupported characters; \
             use letters, digits, dot, dash or underscore"
        )));
    }
    Ok(id.to_string())
}

/// Validate the model name. A provider without a model cannot serve a request.
pub fn validate_model(raw: &str) -> Result<String> {
    let model = raw.trim();
    if model.is_empty() {
        return Err(ArkError::validation(
            "model is required; set the model name this provider should call",
        ));
    }
    Ok(model.to_string())
}

/// Parse `endpoint_format` through the shared adapter so a typo cannot silently
/// pick a wire family. A blank value means the default family.
pub fn validate_endpoint_format(raw: &str) -> Result<EndpointFormat> {
    EndpointFormat::parse(raw).map_err(|_| {
        ArkError::validation(format!(
            "unsupported endpoint format `{}`; expected openai_chat, openai_responses \
             or anthropic_messages",
            raw.trim()
        ))
    })
}

/// `base_url` is optional. When present it must be an absolute http(s) URL; the
/// path/version appending is left to `ark-llm`, which owns the wire family.
pub fn validate_base_url(raw: &str) -> Result<String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    let parsed = url::Url::parse(value).map_err(|err| {
        ArkError::validation(format!(
            "base URL `{}` is not a valid URL: {err}",
            crate::domain::redact_url(value)
        ))
    })?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(ArkError::validation(format!(
                "base URL must start with http:// or https://, but its scheme is `{other}`"
            )))
        }
    }
    if parsed.host_str().map(str::is_empty).unwrap_or(true) {
        return Err(ArkError::validation("base URL must include a host name"));
    }
    Ok(value.trim_end_matches('/').to_string())
}

/// `temperature` is optional; when present it must stay in `0..=2`. A NaN value
/// is rejected as well, because it compares false against every bound.
pub fn validate_temperature(value: Option<f32>) -> Result<Option<f32>> {
    let Some(temp) = value else {
        return Ok(None);
    };
    if !temp.is_finite() || !(0.0..=2.0).contains(&temp) {
        return Err(ArkError::validation(format!(
            "temperature must be between 0 and 2, got {temp}"
        )));
    }
    Ok(Some(temp))
}

/// `max_tokens` is optional; zero is meaningless on the wire.
pub fn validate_max_tokens(value: Option<u32>) -> Result<Option<u32>> {
    match value {
        Some(0) => Err(ArkError::validation(
            "max_tokens must be greater than 0 when set",
        )),
        other => Ok(other),
    }
}

/// Validate and canonicalize a whole entry.
///
/// The returned entry has trimmed fields and a canonical `endpoint_format`
/// string, so persisting the same input twice writes identical bytes.
pub fn normalize_entry(entry: ProviderEntry) -> Result<ProviderEntry> {
    let id = validate_id(&entry.id)?;
    let model = validate_model(&entry.model)?;
    let format = validate_endpoint_format(&entry.endpoint_format)?;
    let base_url = validate_base_url(&entry.base_url)?;
    let temperature = validate_temperature(entry.temperature)?;
    let max_tokens = validate_max_tokens(entry.max_tokens)?;
    let label = entry
        .label
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let api_key = entry
        .api_key
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    Ok(ProviderEntry {
        id,
        label,
        base_url,
        api_key,
        endpoint_format: format.as_str().to_string(),
        model,
        enabled: entry.enabled,
        headers: entry.headers,
        max_tokens,
        temperature,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_restricted_to_the_documented_alphabet() {
        assert_eq!(validate_id("deepseek").unwrap(), "deepseek");
        assert_eq!(validate_id(" qwen-local_1.2 ").unwrap(), "qwen-local_1.2");
        for bad in ["", "   ", "has space", "sla/sh", "a?b", "emoji-🙂"] {
            let err = validate_id(bad).unwrap_err();
            assert_eq!(err.code, ark_common::ArkErrorCode::ValidationError, "{bad}");
        }
    }

    #[test]
    fn ids_longer_than_the_limit_are_rejected() {
        let ok = "a".repeat(ID_MAX_LEN);
        assert!(validate_id(&ok).is_ok());
        let too_long = "a".repeat(ID_MAX_LEN + 1);
        let err = validate_id(&too_long).unwrap_err();
        assert!(err.message.contains("too long"), "{}", err.message);
    }

    #[test]
    fn model_is_required() {
        assert_eq!(validate_model(" deepseek-chat ").unwrap(), "deepseek-chat");
        let err = validate_model("  ").unwrap_err();
        assert!(err.message.contains("model is required"));
    }

    #[test]
    fn unknown_endpoint_format_lists_the_valid_values() {
        let err = validate_endpoint_format("grpc").unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::ValidationError);
        assert!(err.message.contains("openai_chat"), "{}", err.message);
        assert!(err.message.contains("openai_responses"), "{}", err.message);
        assert!(
            err.message.contains("anthropic_messages"),
            "{}",
            err.message
        );
    }

    #[test]
    fn endpoint_format_accepts_aliases_and_blank() {
        assert_eq!(
            validate_endpoint_format("").unwrap(),
            EndpointFormat::OpenAiChat
        );
        assert_eq!(
            validate_endpoint_format("anthropic").unwrap(),
            EndpointFormat::AnthropicMessages
        );
    }

    #[test]
    fn base_url_must_be_absolute_http() {
        assert_eq!(
            validate_base_url(" https://api.deepseek.com/ ").unwrap(),
            "https://api.deepseek.com"
        );
        assert_eq!(validate_base_url("").unwrap(), "");
        for bad in ["api.deepseek.com", "ftp://x.local", "https://"] {
            let err = validate_base_url(bad).unwrap_err();
            assert_eq!(
                err.code,
                ark_common::ArkErrorCode::ValidationError,
                "{bad} should fail"
            );
        }
    }

    #[test]
    fn a_base_url_error_does_not_echo_the_query_secret() {
        let err = validate_base_url("https://x local/v1?api_key=sk-secret-value").unwrap_err();
        assert!(!err.message.contains("sk-secret-value"), "{}", err.message);
    }

    #[test]
    fn temperature_is_bounded_to_zero_through_two() {
        assert_eq!(validate_temperature(None).unwrap(), None);
        assert_eq!(validate_temperature(Some(0.0)).unwrap(), Some(0.0));
        assert_eq!(validate_temperature(Some(2.0)).unwrap(), Some(2.0));
        for bad in [-0.1_f32, 2.1, 100.0, f32::NAN] {
            let err = validate_temperature(Some(bad)).unwrap_err();
            assert!(
                err.message.contains("temperature must be between 0 and 2"),
                "{}",
                err.message
            );
        }
    }

    #[test]
    fn zero_max_tokens_is_rejected() {
        assert!(validate_max_tokens(Some(0)).is_err());
        assert_eq!(validate_max_tokens(Some(16)).unwrap(), Some(16));
    }

    #[test]
    fn normalization_trims_and_canonicalizes() {
        let mut entry = ProviderEntry::new(" deepseek ", " deepseek-chat ");
        entry.base_url = " https://api.deepseek.com/ ".into();
        entry.endpoint_format = "chat".into();
        entry.label = Some("  ".into());
        let out = normalize_entry(entry).unwrap();
        assert_eq!(out.id, "deepseek");
        assert_eq!(out.model, "deepseek-chat");
        assert_eq!(out.base_url, "https://api.deepseek.com");
        assert_eq!(out.endpoint_format, "openai_chat");
        assert_eq!(out.label, None);
    }
}
