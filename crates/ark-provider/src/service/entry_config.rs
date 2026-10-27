//! Mapping from a stored entry to the client configuration `ark-llm` needs.
//!
//! Kept separate from the store so the "is this entry usable" decision is one
//! small function with its own tests, instead of an `if` chain buried in
//! `resolve`.

use ark_contract::ProviderEntry;
use ark_llm::{EndpointFormat, ProviderConfig};

/// Build a usable [`ProviderConfig`], or `None` when the entry could not serve a
/// request.
///
/// `None` means one of: disabled, blank model, missing or blank api key. The
/// caller must not fall back to an empty key — that turns a configuration
/// mistake into a confusing 401 from the provider.
pub fn config_from_entry(entry: &ProviderEntry) -> Option<ProviderConfig> {
    if !entry.enabled {
        return None;
    }
    let model = entry.model.trim();
    if model.is_empty() {
        return None;
    }
    let api_key = entry.api_key.as_deref().unwrap_or("").trim();
    if api_key.is_empty() {
        return None;
    }
    let format = EndpointFormat::parse(&entry.endpoint_format).unwrap_or_default();
    let mut config = ProviderConfig::new(entry.id.clone(), model)
        .with_base_url(entry.base_url.trim())
        .with_api_key(api_key)
        .with_format(format);
    config.max_tokens = entry.max_tokens;
    config.temperature = entry.temperature;
    Some(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> ProviderEntry {
        let mut entry = ProviderEntry::new("deepseek", "deepseek-chat");
        entry.base_url = "https://api.deepseek.com".into();
        entry.api_key = Some("sk-1234567890abcdef".into());
        entry.endpoint_format = "openai_chat".into();
        entry
    }

    #[test]
    fn a_complete_entry_maps_to_a_config() {
        let config = config_from_entry(&entry()).unwrap();
        assert_eq!(config.provider_id, "deepseek");
        assert_eq!(config.model, "deepseek-chat");
        assert_eq!(config.api_key, "sk-1234567890abcdef");
        assert_eq!(config.endpoint_format, EndpointFormat::OpenAiChat);
        assert_eq!(
            config.request_url(),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn a_missing_key_is_not_papered_over_with_an_empty_one() {
        let mut blank = entry();
        blank.api_key = None;
        assert!(config_from_entry(&blank).is_none());
        blank.api_key = Some("   ".into());
        assert!(config_from_entry(&blank).is_none());
    }

    #[test]
    fn a_disabled_entry_never_resolves() {
        let mut disabled = entry();
        disabled.enabled = false;
        assert!(config_from_entry(&disabled).is_none());
    }

    #[test]
    fn a_blank_model_never_resolves() {
        let mut blank = entry();
        blank.model = "  ".into();
        assert!(config_from_entry(&blank).is_none());
    }

    #[test]
    fn optional_knobs_are_carried_over() {
        let mut tuned = entry();
        tuned.max_tokens = Some(512);
        tuned.temperature = Some(0.2);
        let config = config_from_entry(&tuned).unwrap();
        assert_eq!(config.max_tokens, Some(512));
        assert_eq!(config.temperature, Some(0.2));
    }

    #[test]
    fn an_unknown_stored_format_falls_back_to_the_default_family() {
        let mut odd = entry();
        odd.endpoint_format = "grpc".into();
        let config = config_from_entry(&odd).unwrap();
        assert_eq!(config.endpoint_format, EndpointFormat::OpenAiChat);
    }
}
