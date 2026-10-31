//! Canonical JSON schemas for the builtin tools (design §7.3).
//!
//! Each schema is built once and leaked into a `OnceLock`, so `Tool::schema`
//! can hand out a `&'static Value` without locking on every model round. The
//! fingerprint is computed from the canonical encoding, which is what keeps the
//! prompt-cache prefix byte-stable between turns.

use std::sync::OnceLock;

/// Canonical encoding: object keys sorted, no whitespace.
pub fn canonical(value: &serde_json::Value) -> String {
    let sorted = sort_value(value);
    serde_json::to_string(&sorted).unwrap_or_default()
}

fn sort_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for key in keys {
                out.insert(key.clone(), sort_value(&map[key]));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(sort_value).collect())
        }
        other => other.clone(),
    }
}

/// Stable 64-bit FNV-1a fingerprint of the canonical schema text.
pub fn fingerprint(schema: &serde_json::Value) -> u64 {
    fingerprint_text(&canonical(schema))
}

/// FNV-1a over bytes. Stable across runs, unlike `DefaultHasher`.
pub fn fingerprint_text(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn object(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn read_schema() -> serde_json::Value {
    object(
        serde_json::json!({
            "path": { "type": "string", "description": "File path, absolute or relative to the workspace" },
            "offset": { "type": "integer", "minimum": 0, "description": "First line to read (0-based)" },
            "limit": { "type": "integer", "minimum": 1, "description": "Maximum number of lines" },
        }),
        &["path"],
    )
}

fn write_schema() -> serde_json::Value {
    object(
        serde_json::json!({
            "path": { "type": "string", "description": "File path inside the workspace" },
            "content": { "type": "string", "description": "Complete new file content" },
        }),
        &["path", "content"],
    )
}

fn edit_schema() -> serde_json::Value {
    object(
        serde_json::json!({
            "path": { "type": "string", "description": "File path inside the workspace" },
            "old_string": { "type": "string", "description": "Exact text to replace" },
            "new_string": { "type": "string", "description": "Replacement text" },
            "replace_all": { "type": "boolean", "description": "Replace every occurrence" },
        }),
        &["path", "old_string", "new_string"],
    )
}

fn ls_schema() -> serde_json::Value {
    object(
        serde_json::json!({
            "path": { "type": "string", "description": "Directory path, defaults to the workspace root" },
            "limit": { "type": "integer", "minimum": 1, "description": "Maximum entries" },
        }),
        &[],
    )
}

fn find_schema() -> serde_json::Value {
    object(
        serde_json::json!({
            "pattern": { "type": "string", "description": "Glob such as *.rs or src/**/*.ts" },
            "path": { "type": "string", "description": "Directory to search below" },
            "limit": { "type": "integer", "minimum": 1, "description": "Maximum results" },
        }),
        &["pattern"],
    )
}

fn grep_schema() -> serde_json::Value {
    object(
        serde_json::json!({
            "pattern": { "type": "string", "description": "Text or regular expression to search" },
            "path": { "type": "string", "description": "Directory or file to search" },
            "glob": { "type": "string", "description": "Only search files matching this glob" },
            "ignore_case": { "type": "boolean", "description": "Case-insensitive match" },
            "literal": { "type": "boolean", "description": "Treat the pattern as plain text" },
            "context": { "type": "integer", "minimum": 0, "description": "Context lines around a match" },
            "limit": { "type": "integer", "minimum": 1, "description": "Maximum matching lines" },
        }),
        &["pattern"],
    )
}

fn bash_schema() -> serde_json::Value {
    object(
        serde_json::json!({
            "command": { "type": "string", "description": "Shell command to run" },
            "timeout_ms": { "type": "integer", "minimum": 1, "description": "Hard timeout in milliseconds" },
            "cwd": { "type": "string", "description": "Working directory inside the workspace" },
        }),
        &["command"],
    )
}

fn delegate_task_schema() -> serde_json::Value {
    object(
        serde_json::json!({
            "prompt": { "type": "string", "description": "The complete task for the child run; it sees only this, so state the goal and the boundaries" },
            "label": { "type": "string", "description": "Short human label shown in the parent thread" },
            "model": { "type": "string", "description": "Model override; defaults to the parent turn's model" },
            "provider_id": { "type": "string", "description": "Provider override; defaults to the parent turn's provider" },
            "autonomy": { "type": "string", "enum": ["strict", "workspace_write", "unattended"], "description": "Permission override; defaults to the parent thread's permission" },
            "background": { "type": "boolean", "description": "Return after the child is queued and keep it running under the runtime supervisor" },
            "resumeChildId": { "type": "string", "description": "Resume this interrupted child on its existing session instead of creating a new child" },
        }),
        &["prompt"],
    )
}

fn request_user_input_schema() -> serde_json::Value {
    object(
        serde_json::json!({
            "prompt": { "type": "string", "description": "The question shown to the user" },
        }),
        &["prompt"],
    )
}

macro_rules! static_schema {
    ($fn_name:ident, $builder:ident) => {
        pub fn $fn_name() -> &'static serde_json::Value {
            static CELL: OnceLock<serde_json::Value> = OnceLock::new();
            CELL.get_or_init($builder)
        }
    };
}

static_schema!(read, read_schema);
static_schema!(write, write_schema);
static_schema!(edit, edit_schema);
static_schema!(ls, ls_schema);
static_schema!(find, find_schema);
static_schema!(grep, grep_schema);
static_schema!(bash, bash_schema);
static_schema!(delegate_task, delegate_task_schema);
static_schema!(request_user_input, request_user_input_schema);

/// Schema for `name`, or `None` when it is not a builtin.
pub fn schema_of(name: &str) -> Option<&'static serde_json::Value> {
    match name {
        "bash" => Some(bash()),
        "delegate_task" => Some(delegate_task()),
        "edit" => Some(edit()),
        "find" => Some(find()),
        "grep" => Some(grep()),
        "ls" => Some(ls()),
        "read" => Some(read()),
        "request_user_input" => Some(request_user_input()),
        "write" => Some(write()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_encoding_sorts_keys() {
        let a = serde_json::json!({ "b": 1, "a": 2 });
        let b = serde_json::json!({ "a": 2, "b": 1 });
        assert_eq!(canonical(&a), canonical(&b));
        assert_eq!(canonical(&a), "{\"a\":2,\"b\":1}");
    }

    #[test]
    fn fingerprints_are_stable_across_calls() {
        assert_eq!(fingerprint(read()), fingerprint(read()));
        assert_ne!(fingerprint(read()), fingerprint(write()));
    }

    #[test]
    fn every_builtin_schema_declares_an_object_with_required_fields() {
        for name in ["bash", "edit", "find", "grep", "ls", "read", "write"] {
            let schema = schema_of(name).unwrap();
            assert_eq!(schema["type"], "object");
            assert!(schema["properties"].is_object(), "{name} has no properties");
        }
    }

    #[test]
    fn unknown_tool_has_no_schema() {
        assert!(schema_of("nope").is_none());
    }

    #[test]
    fn fnv_is_deterministic() {
        assert_eq!(fingerprint_text("abc"), fingerprint_text("abc"));
        assert_ne!(fingerprint_text("abc"), fingerprint_text("abd"));
    }
}
