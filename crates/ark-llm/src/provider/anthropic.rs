//! Anthropic messages family: `POST {base}/messages`.
//!
//! Stream shape: `message_start` (input usage), `content_block_start`,
//! `content_block_delta` (`text_delta` / `input_json_delta`),
//! `content_block_stop`, `message_delta` (output usage), `message_stop`.
//!
//! Unlike the OpenAI families, tool arguments stream as `input_json_delta`
//! fragments keyed by the *content block* index, and the tool name/call id only
//! appear on `content_block_start`.

use ark_common::{ArkError, Result, Usage};
use ark_ports::{ModelMessage, ModelRequest, ModelRole};
use serde_json::{json, Value};

use crate::common::sse::SseEvent;
use crate::common::usage::UsageParser;
use crate::config::endpoint::ProviderConfig;
use crate::domain::event::{StreamEvent, ToolFragment};
use crate::provider::StreamParser;

/// Anthropic requires `max_tokens`; a request without one is a caller bug.
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Pinned Messages API version. Sent as `anthropic-version` on every request.
pub const API_VERSION: &str = "2023-06-01";

/// Build the JSON body for one call.
///
/// System messages are hoisted out of `messages` into the top-level `system`
/// field, which is what the Messages API expects.
pub fn build_body(config: &ProviderConfig, request: &ModelRequest) -> Result<Value> {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    for message in &request.messages {
        if message.role == ModelRole::System {
            if !message.content.trim().is_empty() {
                system_parts.push(message.content.clone());
            }
            continue;
        }
        messages.push(message_json(message));
    }
    if messages.is_empty() {
        return Err(ArkError::validation(
            "anthropic request needs at least one non-system message",
        ));
    }
    let max_tokens = request
        .max_tokens
        .or(config.max_tokens)
        .unwrap_or(DEFAULT_MAX_TOKENS);
    let mut body = json!({
        "model": config.model,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": true,
    });
    if !system_parts.is_empty() {
        body["system"] = json!(system_parts.join("\n\n"));
    }
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|schema| {
                    json!({
                        "name": schema.get("name").cloned().unwrap_or(Value::Null),
                        "description": schema.get("description").cloned().unwrap_or(json!("")),
                        "input_schema": schema.get("parameters").cloned().unwrap_or(json!({"type": "object"})),
                    })
                })
                .collect(),
        );
    }
    if let Some(temperature) = request.temperature.or(config.temperature) {
        body["temperature"] = json!(temperature);
    }
    Ok(body)
}

fn message_json(message: &ModelMessage) -> Value {
    match message.role {
        ModelRole::Tool => json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
                "content": message.content,
            }],
        }),
        ModelRole::Assistant => {
            // Replayed calls ride along as tool_use blocks; the paired
            // tool_result block arrives in the following user message.
            let mut content = vec![json!({ "type": "text", "text": message.content })];
            content.extend(message.tool_calls.iter().map(|call| {
                json!({
                    "type": "tool_use",
                    "id": call.call_id,
                    "name": call.name,
                    "input": call.arguments,
                })
            }));
            json!({ "role": "assistant", "content": content })
        }
        _ => json!({
            "role": "user",
            "content": [{ "type": "text", "text": message.content }],
        }),
    }
}

/// Streaming frame parser for this family.
#[derive(Debug, Default)]
pub struct AnthropicParser {
    /// Open tool blocks: block index -> (call id, tool name).
    blocks: Vec<Option<(String, String)>>,
    /// Usage from `message_start`. `message_delta` only carries the output
    /// side, so the two must be merged rather than overwritten.
    start_usage: Option<Value>,
}

impl AnthropicParser {
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&mut self, index: usize) -> &mut Option<(String, String)> {
        if self.blocks.len() <= index {
            self.blocks.resize(index + 1, None);
        }
        &mut self.blocks[index]
    }
}

impl StreamParser for AnthropicParser {
    fn parse(&mut self, event: &SseEvent) -> Vec<StreamEvent> {
        if event.is_done() {
            return vec![StreamEvent::Terminal];
        }
        let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
            return vec![StreamEvent::Ignored];
        };
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| event.name());
        match kind {
            "message_start" => self.message_start(&value),
            "content_block_start" => self.content_block_start(&value),
            "content_block_delta" => self.content_block_delta(&value),
            "message_delta" => self.message_delta(&value),
            "message_stop" => vec![StreamEvent::Terminal],
            "ping" | "error" => vec![StreamEvent::Ignored],
            _ => vec![StreamEvent::Ignored],
        }
    }
}

impl AnthropicParser {
    fn message_start(&mut self, value: &Value) -> Vec<StreamEvent> {
        let Some(usage) = value.get("message").and_then(|m| m.get("usage")) else {
            return vec![StreamEvent::Ignored];
        };
        self.start_usage = Some(usage.clone());
        let parsed = UsageParser::new().parse_anthropic(usage);
        vec![StreamEvent::Usage(parsed)]
    }

    fn content_block_start(&mut self, value: &Value) -> Vec<StreamEvent> {
        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let block = value.get("content_block").cloned().unwrap_or(Value::Null);
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                let call_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("toolu_0")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                *self.slot(index) = Some((call_id.clone(), name.clone()));
                vec![StreamEvent::Tool(
                    ToolFragment::new(index as u32)
                        .with_call_id(call_id)
                        .with_name(name),
                )]
            }
            Some("text") => {
                // Some providers include the first text slice inline.
                match block.get("text").and_then(Value::as_str) {
                    Some(text) if !text.is_empty() => vec![StreamEvent::Text(text.to_string())],
                    _ => vec![StreamEvent::Ignored],
                }
            }
            _ => vec![StreamEvent::Ignored],
        }
    }

    fn content_block_delta(&mut self, value: &Value) -> Vec<StreamEvent> {
        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let Some(delta) = value.get("delta") else {
            return vec![StreamEvent::Ignored];
        };
        match delta.get("type").and_then(Value::as_str) {
            Some("text_delta") => match delta.get("text").and_then(Value::as_str) {
                Some(text) if !text.is_empty() => vec![StreamEvent::Text(text.to_string())],
                _ => vec![StreamEvent::Ignored],
            },
            Some("thinking_delta") => match delta.get("thinking").and_then(Value::as_str) {
                Some(text) if !text.is_empty() => vec![StreamEvent::Reasoning(text.to_string())],
                _ => vec![StreamEvent::Ignored],
            },
            Some("input_json_delta") => {
                let partial = delta
                    .get("partial_json")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mut fragment = ToolFragment::new(index as u32).with_arguments(partial);
                if let Some(Some((call_id, _))) = self.blocks.get(index) {
                    fragment = fragment.with_call_id(call_id.clone());
                }
                vec![StreamEvent::Tool(fragment)]
            }
            _ => vec![StreamEvent::Ignored],
        }
    }

    fn message_delta(&mut self, value: &Value) -> Vec<StreamEvent> {
        let Some(usage) = value.get("usage") else {
            return vec![StreamEvent::Ignored];
        };
        // `message_delta.usage` only carries output tokens; the input and cache
        // counters came from `message_start`, so merge field by field.
        let mut merged = self
            .start_usage
            .clone()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        if let (Some(target), Some(source)) = (merged.as_object_mut(), usage.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        vec![StreamEvent::Usage(
            UsageParser::new().parse_anthropic(&merged),
        )]
    }
}

/// Usage for a non-streamed messages payload.
pub fn parse_usage(usage: &Value) -> Usage {
    UsageParser::new().parse_anthropic(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ProviderConfig {
        ProviderConfig::new("anthropic", "claude-sonnet-4")
            .with_format(crate::config::EndpointFormat::AnthropicMessages)
    }

    fn data(value: Value) -> SseEvent {
        SseEvent {
            event: None,
            data: value.to_string(),
        }
    }

    #[test]
    fn system_prompt_is_hoisted_and_max_tokens_is_defaulted() {
        let request = ModelRequest {
            messages: vec![ModelMessage::system("be brief"), ModelMessage::user("hi")],
            ..Default::default()
        };
        let body = build_body(&config(), &request).unwrap();
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn tools_use_input_schema() {
        let request = ModelRequest {
            messages: vec![ModelMessage::user("hi")],
            tools: vec![
                json!({"name": "read", "description": "read a file", "parameters": {"type": "object"}}),
                // Function shape as produced by the tool registry: the JSON
                // Schema travels under `parameters`, `description` only when
                // the tool declares one.
                json!({"name": "bash", "parameters": {"type": "object", "properties": {"command": {"type": "string"}}}}),
            ],
            ..Default::default()
        };
        let body = build_body(&config(), &request).unwrap();
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert!(body["tools"][0].get("function").is_none());
        assert_eq!(
            body["tools"][1]["input_schema"]["properties"]["command"]["type"],
            "string"
        );
    }

    #[test]
    fn tool_result_is_wrapped_as_a_user_block() {
        let request = ModelRequest {
            messages: vec![ModelMessage {
                role: ModelRole::Tool,
                content: "body".into(),
                tool_call_id: Some("toolu_1".into()),
                name: None,
                tool_calls: Vec::new(),
            }],
            ..Default::default()
        };
        let body = build_body(&config(), &request).unwrap();
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][0]["content"][0]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn assistant_tool_calls_serialise_as_tool_use_blocks() {
        let request = ModelRequest {
            messages: vec![ModelMessage::assistant_with_tool_calls(
                "calling",
                vec![ark_ports::ToolCallRef {
                    call_id: "toolu_1".into(),
                    name: "read".into(),
                    arguments: serde_json::json!({"path": "a.txt"}),
                }],
            )],
            ..Default::default()
        };
        let body = build_body(&config(), &request).unwrap();
        let blocks = &body["messages"][0]["content"];
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "toolu_1");
        assert_eq!(blocks[1]["name"], "read");
        assert_eq!(blocks[1]["input"], serde_json::json!({"path": "a.txt"}));
    }

    #[test]
    fn system_only_request_is_rejected() {
        let request = ModelRequest {
            messages: vec![ModelMessage::system("only")],
            ..Default::default()
        };
        assert!(build_body(&config(), &request).is_err());
    }

    #[test]
    fn text_delta_is_parsed() {
        let mut parser = AnthropicParser::new();
        let events = parser.parse(&data(json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "he"}
        })));
        assert_eq!(events, vec![StreamEvent::Text("he".into())]);
    }

    #[test]
    fn thinking_delta_becomes_reasoning() {
        let mut parser = AnthropicParser::new();
        let events = parser.parse(&data(json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "hmm"}
        })));
        assert_eq!(events, vec![StreamEvent::Reasoning("hmm".into())]);
    }

    #[test]
    fn tool_use_block_plus_json_deltas_assemble_a_call() {
        let mut parser = AnthropicParser::new();
        let start = parser.parse(&data(json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {"type": "tool_use", "id": "toolu_9", "name": "read"}
        })));
        match &start[0] {
            StreamEvent::Tool(fragment) => {
                assert_eq!(fragment.call_id.as_deref(), Some("toolu_9"));
                assert_eq!(fragment.name.as_deref(), Some("read"));
            }
            other => panic!("expected tool fragment, got {other:?}"),
        }
        let delta = parser.parse(&data(json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{\"path\":\"a\"}"}
        })));
        match &delta[0] {
            StreamEvent::Tool(fragment) => {
                assert_eq!(fragment.index, 1);
                assert_eq!(fragment.call_id.as_deref(), Some("toolu_9"));
                assert_eq!(fragment.arguments_delta, "{\"path\":\"a\"}");
            }
            other => panic!("expected tool fragment, got {other:?}"),
        }
    }

    #[test]
    fn message_start_reports_input_usage() {
        let mut parser = AnthropicParser::new();
        let events = parser.parse(&data(json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 100, "output_tokens": 1}}
        })));
        match &events[0] {
            StreamEvent::Usage(usage) => assert_eq!(usage.prompt_tokens, 100),
            other => panic!("expected usage, got {other:?}"),
        }
    }

    #[test]
    fn message_delta_merges_the_input_side_from_message_start() {
        let mut parser = AnthropicParser::new();
        parser.parse(&data(json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 40, "output_tokens": 0, "cache_read_input_tokens": 60}}
        })));
        let events = parser.parse(&data(json!({
            "type": "message_delta",
            "usage": {"output_tokens": 7}
        })));
        match &events[0] {
            StreamEvent::Usage(usage) => {
                assert_eq!(usage.prompt_tokens, 40);
                assert_eq!(usage.completion_tokens, 7);
                assert_eq!(usage.cache_hit_tokens, Some(60));
            }
            other => panic!("expected usage, got {other:?}"),
        }
    }

    #[test]
    fn message_stop_is_terminal() {
        let mut parser = AnthropicParser::new();
        assert_eq!(
            parser.parse(&data(json!({"type": "message_stop"}))),
            vec![StreamEvent::Terminal]
        );
    }

    #[test]
    fn ping_is_ignored() {
        let mut parser = AnthropicParser::new();
        assert_eq!(
            parser.parse(&data(json!({"type": "ping"}))),
            vec![StreamEvent::Ignored]
        );
    }

    #[test]
    fn inline_text_on_block_start_is_forwarded() {
        let mut parser = AnthropicParser::new();
        let events = parser.parse(&data(json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": "hello"}
        })));
        assert_eq!(events, vec![StreamEvent::Text("hello".into())]);
    }
}
