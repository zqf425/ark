//! One tool call as the agent sees it, and the outcome of running it.

use serde::{Deserialize, Serialize};

/// A complete model-issued tool call. `arguments` is already parsed JSON, so no
/// layer below has to re-parse a string (design §7.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub tool: String,
    pub arguments: serde_json::Value,
}

impl ToolCallRequest {
    pub fn new(
        call_id: impl Into<String>,
        tool: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            tool: tool.into(),
            arguments,
        }
    }

    /// Stable key for storm detection and parallel-safety: tool name plus the
    /// canonical argument text.
    pub fn signature(&self) -> String {
        format!("{}:{}", self.tool, canonical_json(&self.arguments))
    }

    /// The resource this call touches, used by the parallel-safety policy.
    pub fn resource(&self) -> Option<&str> {
        for key in ["path", "file_path", "filePath", "target"] {
            if let Some(value) = self.arguments.get(key).and_then(|v| v.as_str()) {
                return Some(value);
            }
        }
        None
    }
}

/// Deterministic JSON rendering: object keys sorted, no whitespace.
pub fn canonical_json(value: &serde_json::Value) -> String {
    fn walk(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push('{');
                for (idx, key) in keys.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::String((**key).clone()).to_string());
                    out.push(':');
                    walk(&map[*key], out);
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (idx, item) in items.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    walk(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }

    let mut out = String::new();
    walk(value, &mut out);
    out
}

/// What actually happened when the reactor ran one call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecution {
    pub call_id: String,
    pub tool: String,
    pub ok: bool,
    pub content: String,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ToolExecution {
    pub fn ok(call: &ToolCallRequest, content: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            call_id: call.call_id.clone(),
            tool: call.tool.clone(),
            ok: true,
            content: content.into(),
            duration_ms,
            detail: None,
        }
    }

    pub fn failed(call: &ToolCallRequest, content: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            call_id: call.call_id.clone(),
            tool: call.tool.clone(),
            ok: false,
            content: content.into(),
            duration_ms,
            detail: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_key_order_independent() {
        let a = ToolCallRequest::new(
            "c1",
            "read",
            serde_json::json!({"path": "a.txt", "offset": 3}),
        );
        let b = ToolCallRequest::new(
            "c2",
            "read",
            serde_json::json!({"offset": 3, "path": "a.txt"}),
        );
        assert_eq!(a.signature(), b.signature());
    }

    #[test]
    fn signature_separates_different_tools() {
        let a = ToolCallRequest::new("c1", "read", serde_json::json!({"path": "a.txt"}));
        let b = ToolCallRequest::new("c1", "write", serde_json::json!({"path": "a.txt"}));
        assert_ne!(a.signature(), b.signature());
    }

    #[test]
    fn resource_reads_the_common_argument_names() {
        let call = ToolCallRequest::new("c1", "read", serde_json::json!({"file_path": "x.rs"}));
        assert_eq!(call.resource(), Some("x.rs"));
        let none = ToolCallRequest::new("c2", "list", serde_json::json!({}));
        assert_eq!(none.resource(), None);
    }
}
