//! OpenAI chat-completions family: `POST {base}/chat/completions`.
//!
//! Stream shape: one SSE frame per chunk, content under
//! `choices[0].delta.content`, reasoning under `delta.reasoning_content`
//! (DeepSeek), tool calls under `choices[0].delta.tool_calls[]` with
//! `index`, `id`, `function.name` and `function.arguments` fragments. Usage
//! arrives on the last chunk of the stream as a sibling `usage` object.

use ark_common::{ArkError, Result, Usage};
use ark_ports::{ModelMessage, ModelRequest, ModelRole};
use serde_json::{json, Value};

use crate::common::sse::SseEvent;
use crate::common::usage::UsageParser;
use crate::config::endpoint::ProviderConfig;
use crate::domain::event::{StreamEvent, ToolFragment};
use crate::provider::StreamParser;

/// Build the JSON body for one call.
pub fn build_body(config: &ProviderConfig, request: &ModelRequest) -> Result<Value> {
    let messages = request
        .messages
        .iter()
        .map(message_json)
        .collect::<Vec<Value>>();
    let mut body = json!({
        "model": config.model,
        "messages": messages,
        "stream": true,
    });
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|schema| json!({ "type": "function", "function": schema }))
                .collect(),
        );
    }
    if let Some(temperature) = request.temperature.or(config.temperature) {
        body["temperature"] = json!(temperature);
    }
    if let Some(max_tokens) = request.max_tokens.or(config.max_tokens) {
        body["max_tokens"] = json!(max_tokens);
    }
    if body.get("tools").is_some() {
        // Without this the final usage frame is not sent while streaming.
        body["stream_options"] = json!({ "include_usage": true });
    }
    validate(&body)?;
    Ok(body)
}

fn message_json(message: &ModelMessage) -> Value {
    let role = match message.role {
        ModelRole::System => "system",
        ModelRole::User => "user",
        ModelRole::Assistant => "assistant",
        ModelRole::Tool => "tool",
    };
    let mut value = json!({ "role": role, "content": message.content });
    if !message.tool_calls.is_empty() {
        // Chat-completions wire shape: arguments travel as a JSON string, so
        // the parsed Value must be re-serialised before sending.
        value["tool_calls"] = Value::Array(
            message
                .tool_calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.call_id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": serde_json::to_string(&call.arguments)
                                .unwrap_or_default(),
                        },
                    })
                })
                .collect(),
        );
    }
    if let Some(call_id) = &message.tool_call_id {
        value["tool_call_id"] = json!(call_id);
    }
    if let Some(name) = &message.name {
        value["name"] = json!(name);
    }
    value
}

fn validate(body: &Value) -> Result<()> {
    let has_messages = body
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| !messages.is_empty());
    if !has_messages {
        return Err(ArkError::validation(
            "chat completions request needs at least one message",
        ));
    }
    Ok(())
}

/// Streaming frame parser for this family.
#[derive(Debug, Default)]
pub struct ChatParser {
    saw_usage: bool,
}

impl ChatParser {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StreamParser for ChatParser {
    fn parse(&mut self, event: &SseEvent) -> Vec<StreamEvent> {
        if event.is_done() {
            return vec![StreamEvent::Terminal];
        }
        let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
            return vec![StreamEvent::Ignored];
        };
        // An error object can arrive mid-stream with HTTP 200.
        if value.get("error").is_some() {
            return vec![StreamEvent::Ignored];
        }
        let mut events = Vec::new();
        if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
            events.push(StreamEvent::Usage(UsageParser::new().parse_openai(usage)));
            self.saw_usage = true;
        }
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return events;
        };
        if let Some(delta) = choice.get("delta") {
            push_text(delta, &mut events);
            push_tool_calls(delta, &mut events);
        }
        if events.is_empty() {
            events.push(StreamEvent::Ignored);
        }
        events
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        // A stream that ended without `[DONE]` still has a usable response; the
        // accumulator commits the assembled calls when the sink closes.
        Vec::new()
    }
}

fn push_text(delta: &Value, events: &mut Vec<StreamEvent>) {
    if let Some(text) = delta.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            events.push(StreamEvent::Text(text.to_string()));
        }
    }
    let reasoning = delta
        .get("reasoning_content")
        .or_else(|| delta.get("reasoning"))
        .and_then(Value::as_str);
    if let Some(reasoning) = reasoning {
        if !reasoning.is_empty() {
            events.push(StreamEvent::Reasoning(reasoning.to_string()));
        }
    }
}

fn push_tool_calls(delta: &Value, events: &mut Vec<StreamEvent>) {
    let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) else {
        return;
    };
    for (position, call) in calls.iter().enumerate() {
        // `index` is the authoritative identifier; the array position is only a
        // fallback for providers that omit it.
        let index = call
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as u32)
            .unwrap_or(position as u32);
        let mut fragment = ToolFragment::new(index);
        if let Some(call_id) = call.get("id").and_then(Value::as_str) {
            if !call_id.is_empty() {
                fragment = fragment.with_call_id(call_id);
            }
        }
        if let Some(function) = call.get("function") {
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                if !name.is_empty() {
                    fragment = fragment.with_name(name);
                }
            }
            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                fragment = fragment.with_arguments(arguments);
            }
        }
        events.push(StreamEvent::Tool(fragment));
    }
}

/// Usage for a non-streamed chat response.
pub fn parse_usage(usage: &Value) -> Usage {
    UsageParser::new().parse_openai(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ProviderConfig {
        ProviderConfig::new("deepseek", "deepseek-chat")
    }

    fn request() -> ModelRequest {
        ModelRequest {
            messages: vec![ModelMessage::system("sys"), ModelMessage::user("hi")],
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
    fn body_streams_and_keeps_message_order() {
        let body = build_body(&config(), &request()).unwrap();
        assert_eq!(body["model"], "deepseek-chat");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
    }

    #[test]
    fn tools_are_wrapped_and_enable_usage_chunks() {
        let mut request = request();
        request.tools = vec![json!({ "name": "read", "description": "read a file" })];
        let body = build_body(&config(), &request).unwrap();
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn options_fall_back_to_the_provider_config() {
        let mut config = config();
        config.temperature = Some(0.2);
        config.max_tokens = Some(256);
        let body = build_body(&config, &request()).unwrap();
        // `temperature` is an f32 on the config, so compare numerically rather
        // than bit-for-bit against an f64 literal.
        let temperature = body["temperature"].as_f64().unwrap();
        assert!(
            (temperature - 0.2).abs() < 1e-6,
            "temperature was {temperature}"
        );
        assert_eq!(body["max_tokens"], 256);
    }

    #[test]
    fn empty_messages_are_rejected() {
        let error = build_body(&config(), &ModelRequest::default()).unwrap_err();
        assert_eq!(error.code, ark_common::ArkErrorCode::ValidationError);
    }

    #[test]
    fn text_delta_is_parsed() {
        let mut parser = ChatParser::new();
        let events = parser.parse(&frame("{\"choices\":[{\"delta\":{\"content\":\"he\"}}]}"));
        assert_eq!(events, vec![StreamEvent::Text("he".into())]);
    }

    #[test]
    fn reasoning_content_is_separated_from_text() {
        let mut parser = ChatParser::new();
        let events = parser.parse(&frame(
            "{\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"}}]}",
        ));
        assert_eq!(events, vec![StreamEvent::Reasoning("think".into())]);
    }

    #[test]
    fn tool_call_fragments_keep_the_index() {
        let mut parser = ChatParser::new();
        let first = parser.parse(&frame(
            "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"c2\",\"function\":{\"name\":\"grep\"}}]}}]}",
        ));
        match &first[0] {
            StreamEvent::Tool(fragment) => {
                assert_eq!(fragment.index, 1);
                assert_eq!(fragment.call_id.as_deref(), Some("c2"));
                assert_eq!(fragment.name.as_deref(), Some("grep"));
            }
            other => panic!("expected tool fragment, got {other:?}"),
        }
        let second = parser.parse(&frame(
            "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"arguments\":\"{\\\"a\\\":1}\"}}]}}]}",
        ));
        match &second[0] {
            StreamEvent::Tool(fragment) => assert_eq!(fragment.arguments_delta, "{\"a\":1}"),
            other => panic!("expected tool fragment, got {other:?}"),
        }
    }

    #[test]
    fn usage_frame_is_parsed_with_native_cache_fields() {
        let mut parser = ChatParser::new();
        let events = parser.parse(&frame(
            "{\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2,\"prompt_cache_hit_tokens\":8,\"prompt_cache_miss_tokens\":2}}",
        ));
        match &events[0] {
            StreamEvent::Usage(usage) => {
                assert_eq!(usage.prompt_tokens, 10);
                assert_eq!(usage.cache_hit_rate, Some(0.8));
            }
            other => panic!("expected usage, got {other:?}"),
        }
    }

    #[test]
    fn done_sentinel_is_terminal() {
        let mut parser = ChatParser::new();
        assert_eq!(parser.parse(&frame("[DONE]")), vec![StreamEvent::Terminal]);
    }

    #[test]
    fn malformed_json_is_ignored_not_fatal() {
        let mut parser = ChatParser::new();
        assert_eq!(
            parser.parse(&frame("{\"choices\":")),
            vec![StreamEvent::Ignored]
        );
    }

    #[test]
    fn role_only_first_chunk_is_ignored() {
        let mut parser = ChatParser::new();
        let events = parser.parse(&frame(
            "{\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}",
        ));
        assert_eq!(events, vec![StreamEvent::Ignored]);
    }

    #[test]
    fn tool_messages_carry_the_call_id() {
        let message = ModelMessage {
            role: ModelRole::Tool,
            content: "ok".into(),
            tool_call_id: Some("c1".into()),
            name: Some("read".into()),
            tool_calls: Vec::new(),
        };
        let json = message_json(&message);
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "c1");
        assert_eq!(json["name"], "read");
    }

    #[test]
    fn assistant_tool_calls_serialise_as_function_entries() {
        let message = ModelMessage::assistant_with_tool_calls(
            "calling",
            vec![ark_ports::ToolCallRef {
                call_id: "c1".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "a.txt"}),
            }],
        );
        let json = message_json(&message);
        let calls = &json["tool_calls"];
        assert_eq!(calls[0]["id"], "c1");
        assert_eq!(calls[0]["type"], "function");
        assert_eq!(calls[0]["function"]["name"], "read");
        assert_eq!(calls[0]["function"]["arguments"], r#"{"path":"a.txt"}"#);
    }
}
