//! OpenAI Responses family: `POST {base}/responses`.
//!
//! Stream shape: typed SSE events (`event: response.output_text.delta`,
//! `response.function_call_arguments.delta`, `response.completed`), with usage on
//! the terminal `response.completed` payload under `response.usage`.

use ark_common::{ArkError, Result, Usage};
use ark_ports::{ModelMessage, ModelRequest, ModelRole};
use serde_json::{json, Value};

use crate::common::sse::SseEvent;
use crate::common::usage::UsageParser;
use crate::config::endpoint::ProviderConfig;
use crate::domain::event::{StreamEvent, ToolFragment};
use crate::provider::StreamParser;

/// Build the JSON body for one call.
///
/// The Responses API takes `input` items rather than `messages`, and tool
/// schemas flat (`{type:function,name,parameters}`) instead of nested under
/// `function`.
pub fn build_body(config: &ProviderConfig, request: &ModelRequest) -> Result<Value> {
    let input = request
        .messages
        .iter()
        .flat_map(input_json)
        .collect::<Vec<Value>>();
    let mut body = json!({
        "model": config.model,
        "input": input,
        "stream": true,
    });
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|schema| {
                    json!({
                        "type": "function",
                        "name": schema.get("name").cloned().unwrap_or(Value::Null),
                        "parameters": schema.get("parameters").cloned().unwrap_or(json!({})),
                    })
                })
                .collect(),
        );
    }
    if let Some(temperature) = request.temperature.or(config.temperature) {
        body["temperature"] = json!(temperature);
    }
    if let Some(max_tokens) = request.max_tokens.or(config.max_tokens) {
        body["max_output_tokens"] = json!(max_tokens);
    }
    if body
        .get("input")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        return Err(ArkError::validation(
            "responses request needs at least one input item",
        ));
    }
    Ok(body)
}

fn input_json(message: &ModelMessage) -> Vec<Value> {
    match message.role {
        ModelRole::Tool => vec![json!({
            "type": "function_call_output",
            "call_id": message.tool_call_id.clone().unwrap_or_default(),
            "output": message.content,
        })],
        ModelRole::Assistant if !message.tool_calls.is_empty() => {
            // Responses wire shape: a call becomes its own function_call item
            // after the assistant message, mirroring how the stream emitted it.
            let mut items = vec![json!({
                "role": "assistant",
                "content": message.content,
            })];
            items.extend(message.tool_calls.iter().map(|call| {
                json!({
                    "type": "function_call",
                    "call_id": call.call_id,
                    "name": call.name,
                    "arguments": serde_json::to_string(&call.arguments)
                        .unwrap_or_default(),
                })
            }));
            items
        }
        other => {
            let role = match other {
                ModelRole::System => "system",
                ModelRole::User => "user",
                _ => "assistant",
            };
            vec![json!({ "role": role, "content": message.content })]
        }
    }
}

/// Streaming frame parser for this family.
#[derive(Debug, Default)]
pub struct ResponsesParser {
    /// Item ids are assigned by the server; fragments reference them by index.
    call_ids: Vec<String>,
}

impl ResponsesParser {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure_call_id(&mut self, index: usize) -> String {
        if self.call_ids.len() <= index {
            self.call_ids.resize(index + 1, String::new());
        }
        if self.call_ids[index].is_empty() {
            self.call_ids[index] = format!("call_{index}");
        }
        self.call_ids[index].clone()
    }
}

impl StreamParser for ResponsesParser {
    fn parse(&mut self, event: &SseEvent) -> Vec<StreamEvent> {
        if event.is_done() {
            return vec![StreamEvent::Terminal];
        }
        let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
            return vec![StreamEvent::Ignored];
        };
        // The `type` field is authoritative; the SSE `event:` name can be absent.
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| event.name());
        match kind {
            "response.output_text.delta" => text(&value, "delta"),
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                text(&value, "delta").into_iter().map(reasoning).collect()
            }
            "response.output_item.added" => self.output_item_added(&value),
            "response.function_call_arguments.delta" => self.arguments_delta(&value),
            "response.completed" => self.completed(&value),
            "response.failed" | "error" => vec![StreamEvent::Ignored],
            _ => vec![StreamEvent::Ignored],
        }
    }
}

impl ResponsesParser {
    fn output_item_added(&mut self, value: &Value) -> Vec<StreamEvent> {
        let item = value.get("item").unwrap_or(value);
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return vec![StreamEvent::Ignored];
        }
        let index = value
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let call_id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("call_0")
            .to_string();
        if self.call_ids.len() <= index {
            self.call_ids.resize(index + 1, String::new());
        }
        self.call_ids[index] = call_id.clone();
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        vec![StreamEvent::Tool(
            ToolFragment::new(index as u32)
                .with_call_id(call_id)
                .with_name(name),
        )]
    }

    fn arguments_delta(&mut self, value: &Value) -> Vec<StreamEvent> {
        let index = value
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let delta = value
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let call_id = self.ensure_call_id(index);
        vec![StreamEvent::Tool(
            ToolFragment::new(index as u32)
                .with_call_id(call_id)
                .with_arguments(delta),
        )]
    }

    fn completed(&mut self, value: &Value) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        if let Some(usage) = value.get("response").and_then(|r| r.get("usage")) {
            if !usage.is_null() {
                events.push(StreamEvent::Usage(UsageParser::new().parse_openai(usage)));
            }
        }
        events.push(StreamEvent::Terminal);
        events
    }
}

fn text(value: &Value, key: &str) -> Vec<StreamEvent> {
    match value.get(key).and_then(Value::as_str) {
        Some(delta) if !delta.is_empty() => vec![StreamEvent::Text(delta.to_string())],
        _ => vec![StreamEvent::Ignored],
    }
}

fn reasoning(event: StreamEvent) -> StreamEvent {
    match event {
        StreamEvent::Text(delta) => StreamEvent::Reasoning(delta),
        other => other,
    }
}

/// Usage for a non-streamed responses payload.
pub fn parse_usage(usage: &Value) -> Usage {
    UsageParser::new().parse_openai(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ProviderConfig {
        ProviderConfig::new("openai", "gpt-4o")
            .with_format(crate::config::EndpointFormat::OpenAiResponses)
    }

    fn frame(kind: &str, data: Value) -> SseEvent {
        SseEvent {
            event: Some(kind.to_string()),
            data: data.to_string(),
        }
    }

    fn data_frame(data: Value) -> SseEvent {
        SseEvent {
            event: None,
            data: data.to_string(),
        }
    }

    #[test]
    fn body_uses_input_items_and_flat_tools() {
        let request = ModelRequest {
            messages: vec![ModelMessage::user("hi")],
            tools: vec![json!({"name": "read", "parameters": {"type": "object"}})],
            ..Default::default()
        };
        let body = build_body(&config(), &request).unwrap();
        assert!(body["input"].is_array());
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn tool_results_serialise_as_function_call_output() {
        let mut result = ModelMessage {
            role: ModelRole::Tool,
            content: "body".into(),
            tool_call_id: Some("call_1".into()),
            name: None,
            tool_calls: Vec::new(),
        };
        result.name = None;
        let request = ModelRequest {
            messages: vec![result],
            ..Default::default()
        };
        let body = build_body(&config(), &request).unwrap();
        assert_eq!(body["input"][0]["type"], "function_call_output");
        assert_eq!(body["input"][0]["call_id"], "call_1");
    }

    #[test]
    fn empty_request_is_rejected() {
        assert!(build_body(&config(), &ModelRequest::default()).is_err());
    }

    #[test]
    fn text_delta_is_parsed() {
        let mut parser = ResponsesParser::new();
        let events = parser.parse(&frame(
            "response.output_text.delta",
            json!({"type": "response.output_text.delta", "delta": "he"}),
        ));
        assert_eq!(events, vec![StreamEvent::Text("he".into())]);
    }

    #[test]
    fn reasoning_delta_is_parsed() {
        let mut parser = ResponsesParser::new();
        let events = parser.parse(&data_frame(json!({
            "type": "response.reasoning_summary_text.delta",
            "delta": "thinking"
        })));
        assert_eq!(events, vec![StreamEvent::Reasoning("thinking".into())]);
    }

    #[test]
    fn function_call_is_assembled_from_added_plus_deltas() {
        let mut parser = ResponsesParser::new();
        let added = parser.parse(&data_frame(json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"type": "function_call", "call_id": "call_x", "name": "read"}
        })));
        assert_eq!(added.len(), 1);
        let deltas = parser.parse(&data_frame(json!({
            "type": "response.function_call_arguments.delta",
            "output_index": 0,
            "delta": "{\"path\":\"a\"}"
        })));
        match &deltas[0] {
            StreamEvent::Tool(fragment) => {
                assert_eq!(fragment.call_id.as_deref(), Some("call_x"));
                assert_eq!(fragment.arguments_delta, "{\"path\":\"a\"}");
            }
            other => panic!("expected tool fragment, got {other:?}"),
        }
    }

    #[test]
    fn arguments_delta_without_added_still_has_an_id() {
        let mut parser = ResponsesParser::new();
        let events = parser.parse(&data_frame(json!({
            "type": "response.function_call_arguments.delta",
            "output_index": 2,
            "delta": "{}"
        })));
        match &events[0] {
            StreamEvent::Tool(fragment) => {
                assert_eq!(fragment.index, 2);
                assert!(fragment.call_id.is_some());
            }
            other => panic!("expected tool fragment, got {other:?}"),
        }
    }

    #[test]
    fn completed_carries_usage_then_terminal() {
        let mut parser = ResponsesParser::new();
        let events = parser.parse(&data_frame(json!({
            "type": "response.completed",
            "response": {"usage": {"input_tokens": 10, "output_tokens": 2}}
        })));
        assert_eq!(events.len(), 2);
        assert_eq!(events[1], StreamEvent::Terminal);
        match &events[0] {
            StreamEvent::Usage(usage) => assert_eq!(usage.total_tokens, 12),
            other => panic!("expected usage, got {other:?}"),
        }
    }

    #[test]
    fn failed_response_is_ignored_not_fatal() {
        let mut parser = ResponsesParser::new();
        let events = parser.parse(&data_frame(json!({"type": "response.failed"})));
        assert_eq!(events, vec![StreamEvent::Ignored]);
    }

    #[test]
    fn done_sentinel_is_terminal() {
        let mut parser = ResponsesParser::new();
        let events = parser.parse(&SseEvent {
            event: None,
            data: "[DONE]".into(),
        });
        assert_eq!(events, vec![StreamEvent::Terminal]);
    }

    #[test]
    fn non_json_frame_is_ignored() {
        let mut parser = ResponsesParser::new();
        let events = parser.parse(&SseEvent {
            event: None,
            data: "not json".into(),
        });
        assert_eq!(events, vec![StreamEvent::Ignored]);
    }
    #[test]
    fn assistant_tool_calls_serialise_as_function_call_items() {
        let request = ModelRequest {
            messages: vec![ModelMessage::assistant_with_tool_calls(
                "calling",
                vec![ark_ports::ToolCallRef {
                    call_id: "call_1".into(),
                    name: "read".into(),
                    arguments: serde_json::json!({"path": "a.txt"}),
                }],
            )],
            ..Default::default()
        };
        let body = build_body(&config(), &request).unwrap();
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[0]["content"], "calling");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_1");
        assert_eq!(input[1]["name"], "read");
        assert_eq!(input[1]["arguments"], r#"{"path":"a.txt"}"#);
    }
}
