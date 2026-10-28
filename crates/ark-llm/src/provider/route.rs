//! Endpoint routing (design §7.2).
//!
//! `endpointFormat` is matched **once**, here. Provider modules never inspect it,
//! and the HTTP client never contains a `if format == …` chain: it asks the
//! router for a [`Family`] and delegates. That is what keeps the three wire
//! families from growing into each other.

use crate::common::sse::SseEvent;
use crate::config::endpoint::{EndpointFormat, ProviderConfig};
use crate::domain::event::StreamEvent;
use crate::provider::anthropic::{self, AnthropicParser};
use crate::provider::openai_chat::{self, ChatParser};
use crate::provider::openai_responses::{self, ResponsesParser};
use crate::provider::StreamParser;
use std::collections::BTreeMap;

/// The resolved wire family. Obtained from [`ProviderConfig::endpoint_format`].
pub enum Family {
    Chat(Box<ChatParser>),
    Responses(Box<ResponsesParser>),
    Anthropic(Box<AnthropicParser>),
}

impl Family {
    /// Build the family handler for a config. This is the single switch.
    pub fn of(config: &ProviderConfig) -> Self {
        match config.endpoint_format {
            EndpointFormat::OpenAiChat => Family::Chat(Box::new(ChatParser::new())),
            EndpointFormat::OpenAiResponses => Family::Responses(Box::new(ResponsesParser::new())),
            EndpointFormat::AnthropicMessages => {
                Family::Anthropic(Box::new(AnthropicParser::new()))
            }
        }
    }

    pub fn format(&self) -> EndpointFormat {
        match self {
            Family::Chat(_) => EndpointFormat::OpenAiChat,
            Family::Responses(_) => EndpointFormat::OpenAiResponses,
            Family::Anthropic(_) => EndpointFormat::AnthropicMessages,
        }
    }

    /// Request body for this family.
    pub fn body(
        &self,
        config: &ProviderConfig,
        request: &ark_ports::ModelRequest,
    ) -> ark_common::Result<serde_json::Value> {
        match self {
            Family::Chat(_) => openai_chat::build_body(config, request),
            Family::Responses(_) => openai_responses::build_body(config, request),
            Family::Anthropic(_) => anthropic::build_body(config, request),
        }
    }

    /// Auth headers for this family.
    pub fn headers(&self, config: &ProviderConfig) -> Vec<(String, String)> {
        // Header names are ASCII case-insensitive on the wire. Normalize first
        // and overwrite core keys afterwards so a custom header cannot replace
        // the selected endpoint family's authentication or content type.
        let mut headers: BTreeMap<String, String> = config
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .collect();
        match self {
            Family::Chat(_) | Family::Responses(_) => {
                headers.insert(
                    "authorization".to_string(),
                    format!("Bearer {}", config.api_key),
                );
            }
            Family::Anthropic(_) => {
                headers.insert("x-api-key".to_string(), config.api_key.clone());
                headers.insert(
                    "anthropic-version".to_string(),
                    anthropic::API_VERSION.to_string(),
                );
            }
        }
        headers.insert("content-type".to_string(), "application/json".to_string());
        headers.into_iter().collect()
    }
}

impl StreamParser for Family {
    fn parse(&mut self, event: &SseEvent) -> Vec<StreamEvent> {
        match self {
            Family::Chat(parser) => parser.parse(event),
            Family::Responses(parser) => parser.parse(event),
            Family::Anthropic(parser) => parser.parse(event),
        }
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        match self {
            Family::Chat(parser) => parser.finish(),
            Family::Responses(parser) => parser.finish(),
            Family::Anthropic(parser) => parser.finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ports::ModelMessage;
    use serde_json::json;

    fn config(format: EndpointFormat) -> ProviderConfig {
        ProviderConfig::new("test", "test-model")
            .with_api_key("key")
            .with_format(format)
    }

    fn request() -> ark_ports::ModelRequest {
        ark_ports::ModelRequest {
            messages: vec![ModelMessage::user("hi")],
            ..Default::default()
        }
    }

    fn frame(data: &str) -> SseEvent {
        SseEvent {
            event: None,
            data: data.to_string(),
        }
    }

    #[test]
    fn router_picks_exactly_one_family() {
        for format in [
            EndpointFormat::OpenAiChat,
            EndpointFormat::OpenAiResponses,
            EndpointFormat::AnthropicMessages,
        ] {
            assert_eq!(Family::of(&config(format)).format(), format);
        }
    }

    #[test]
    fn bodies_match_the_family() {
        let chat = Family::of(&config(EndpointFormat::OpenAiChat));
        let body = chat
            .body(&config(EndpointFormat::OpenAiChat), &request())
            .unwrap();
        assert_eq!(body["stream"], true);
        assert!(body.get("messages").is_some());

        let responses = Family::of(&config(EndpointFormat::OpenAiResponses));
        let body = responses
            .body(&config(EndpointFormat::OpenAiResponses), &request())
            .unwrap();
        assert!(body.get("input").is_some());

        let anthropic = Family::of(&config(EndpointFormat::AnthropicMessages));
        let body = anthropic
            .body(&config(EndpointFormat::AnthropicMessages), &request())
            .unwrap();
        assert!(body.get("max_tokens").is_some());
        assert!(
            body.get("system").is_none(),
            "no system message in this request"
        );
    }

    #[test]
    fn anthropic_uses_x_api_key_and_a_version_header() {
        let family = Family::of(&config(EndpointFormat::AnthropicMessages));
        let headers = family.headers(&config(EndpointFormat::AnthropicMessages));
        assert!(headers.iter().any(|(name, _)| name == "x-api-key"));
        assert!(headers
            .iter()
            .any(|(name, value)| name == "anthropic-version" && !value.is_empty()));
        assert!(!headers.iter().any(|(name, _)| name == "authorization"));
    }

    #[test]
    fn openai_families_use_a_bearer_token() {
        for format in [EndpointFormat::OpenAiChat, EndpointFormat::OpenAiResponses] {
            let family = Family::of(&config(format));
            let headers = family.headers(&config(format));
            let auth = headers
                .iter()
                .find(|(name, _)| name == "authorization")
                .unwrap();
            assert_eq!(auth.1, "Bearer key");
        }
    }

    #[test]
    fn provider_headers_are_forwarded_but_cannot_override_core_authentication() {
        let mut config = config(EndpointFormat::OpenAiChat);
        config.headers.insert("X-Tenant".into(), "ark".into());
        config
            .headers
            .insert("Authorization".into(), "wrong".into());

        let headers = Family::of(&config).headers(&config);
        assert!(headers
            .iter()
            .any(|(name, value)| name == "x-tenant" && value == "ark"));
        assert!(headers
            .iter()
            .any(|(name, value)| name == "authorization" && value == "Bearer key"));
    }

    #[test]
    fn chat_frames_route_to_the_chat_parser() {
        let mut family = Family::of(&config(EndpointFormat::OpenAiChat));
        let events = family.parse(&frame(
            &json!({
                "choices": [{ "delta": { "content": "hi" } }]
            })
            .to_string(),
        ));
        assert_eq!(events, vec![StreamEvent::Text("hi".into())]);
    }

    #[test]
    fn anthropic_frames_route_to_the_anthropic_parser() {
        let mut family = Family::of(&config(EndpointFormat::AnthropicMessages));
        let events = family.parse(&frame(
            &json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "hi" }
            })
            .to_string(),
        ));
        assert_eq!(events, vec![StreamEvent::Text("hi".into())]);
    }

    #[test]
    fn responses_frames_route_to_the_responses_parser() {
        let mut family = Family::of(&config(EndpointFormat::OpenAiResponses));
        let events = family.parse(&frame(
            &json!({
                "type": "response.output_text.delta",
                "item_id": "msg_1",
                "delta": "hi"
            })
            .to_string(),
        ));
        assert_eq!(events, vec![StreamEvent::Text("hi".into())]);
    }
}
