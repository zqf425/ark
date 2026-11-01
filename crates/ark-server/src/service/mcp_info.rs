//! The MCP discovery document: one self-describing answer for "what can an
//! external agent do with this runtime" (requirements/mcp-server-surface.md).
//!
//! The tool table in [`mcp_tools`](super::mcp_tools) is the single catalog; this
//! module only shapes it into the wire document and binds the runtime facts
//! (version, whether a bearer token is configured). Execution is never proxied
//! here: every tool points at the REST endpoint it describes.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::mcp_tools;

/// One exposed capability bound to the REST route that executes it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolDoc {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub http: McpHttpBinding,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpHttpBinding {
    pub method: &'static str,
    /// Literal route path with `{placeholder}` path parameters, exactly as the
    /// router registers it, so the consistency test can compare the two.
    pub path: &'static str,
}

/// How a caller authenticates. Typed because "Rust types are the only source of
/// wire truth" applies to this document too, not just the contract DTOs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpAuthInfo {
    /// Whether the runtime was started with `--auth-token`.
    pub required: bool,
    /// The header carrying the bearer value.
    pub header: &'static str,
    pub description: String,
}

impl McpServerInfo {
    /// Render to the JSON shape of `GET /v1/mcp-info`.
    pub fn to_json(&self) -> Value {
        json!({
            "server_name": self.server_name,
            "version": self.version,
            "transport": self.transport,
            "auth": self.auth,
            "tools": self.tools.iter().map(|tool| json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
                "http": { "method": tool.http.method, "path": tool.http.path },
            })).collect::<Vec<_>>(),
        })
    }
}

/// The full discovery document served by `GET /v1/mcp-info`.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerInfo {
    pub server_name: &'static str,
    pub version: String,
    pub transport: &'static str,
    pub auth: McpAuthInfo,
    pub tools: Vec<McpToolDoc>,
}

/// Assemble the document. `auth_required` reflects the runtime configuration:
/// the caller must know whether a token is needed before it can call anything
/// else, so the answer is honest rather than a static guess.
pub fn server_info(auth_required: bool, version: &str) -> McpServerInfo {
    McpServerInfo {
        server_name: "arkd",
        version: version.to_string(),
        transport: "http",
        auth: McpAuthInfo {
            required: auth_required,
            header: "authorization",
            description: "Send `authorization: Bearer <token>` on every /v1 request; the token \
                          is the arkd `--auth-token` value. `required` is false only when the \
                          runtime was started without a token (an explicit local-only \
                          configuration)."
                .to_string(),
        },
        tools: mcp_tools::tools(),
    }
}

impl McpToolDoc {
    pub fn new(
        name: &'static str,
        description: &'static str,
        method: &'static str,
        path: &'static str,
        input_schema: Value,
    ) -> Self {
        Self {
            name,
            description,
            input_schema,
            http: McpHttpBinding { method, path },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimum catalog size per route family, so the table cannot silently
    /// lose a whole family while a bare total would still pass.
    fn family_minimums() -> Vec<(&'static str, usize)> {
        vec![
            ("thread_", 20),
            ("turn_", 4),
            ("schedule_", 5),
            ("memory_", 5),
            ("attachment_", 6),
            ("child_run_", 8),
        ]
    }

    #[test]
    fn document_carries_runtime_facts_and_the_full_catalog() {
        let info = server_info(true, "9.9.9-test");
        let json = info.to_json();
        assert_eq!(json["server_name"], "arkd");
        assert_eq!(json["version"], "9.9.9-test");
        assert_eq!(json["auth"]["required"], true);
        assert_eq!(json["auth"]["header"], "authorization");
        assert!(json["tools"].as_array().expect("tools").len() >= 40);
    }

    #[test]
    fn auth_section_reflects_an_unprotected_runtime() {
        let json = server_info(false, "0.1.0").to_json();
        assert_eq!(json["auth"]["required"], false);
    }

    #[test]
    fn tool_names_are_unique_snake_case() {
        let mut names: Vec<&str> = mcp_tools::tools().iter().map(|tool| tool.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "tool names must be unique");
        assert!(names.iter().all(|name| !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')));
    }

    #[test]
    fn every_route_family_keeps_its_tools() {
        let tools = mcp_tools::tools();
        for (prefix, minimum) in family_minimums() {
            let count = tools
                .iter()
                .filter(|tool| tool.name.starts_with(prefix))
                .count();
            assert!(
                count >= minimum,
                "family {prefix} has {count} tools, expected at least {minimum}"
            );
        }
    }

    #[test]
    fn every_schema_is_an_object() {
        for tool in mcp_tools::tools() {
            assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
            // A schema either lists its properties inline (path parameters and
            // the fields a caller must get right) or names the type that owns
            // the body shape; an undocumented schema is a bug.
            let inline = tool.input_schema["properties"].is_object();
            let references_type = tool.input_schema.to_string().contains("Body shape:");
            assert!(
                inline || references_type,
                "{}: schema must list properties or reference the body type",
                tool.name
            );
            assert!(
                matches!(tool.http.method, "GET" | "POST" | "PATCH" | "DELETE"),
                "{}: unexpected method {}",
                tool.name,
                tool.http.method
            );
            assert!(tool.http.path.starts_with("/v1/"), "{}", tool.name);
            assert!(!tool.description.is_empty(), "{}", tool.name);
        }
    }
}
