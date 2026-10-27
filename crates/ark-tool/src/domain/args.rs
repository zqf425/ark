//! Typed argument structs for every builtin tool (design §7.3).
//!
//! A tool never parses a JSON string: the registry deserializes into one of
//! these structs, so a wrong argument is a typed error instead of a silent
//! `null` reaching the filesystem layer.

use ark_common::{ArkError, ChildId, Result};
use ark_ports::ToolArgs;
use serde::Deserialize;

/// Decode `args` into `T`, reporting the tool name on failure.
pub fn parse<T>(tool: &str, args: &ToolArgs) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(args.0.clone())
        .map_err(|err| ArkError::validation(format!("invalid arguments for `{tool}`: {err}")))
}

/// `read` — one file slice.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadArgs {
    pub path: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `write` — replace a whole file.
#[derive(Debug, Clone, Deserialize)]
pub struct WriteArgs {
    pub path: String,
    #[serde(default)]
    pub content: String,
}

/// `edit` — exact text replacement.
#[derive(Debug, Clone, Deserialize)]
pub struct EditArgs {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: Option<bool>,
}

/// `ls` — one directory level.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LsArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `find` — glob search below a directory.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FindArgs {
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `grep` — line-oriented content search.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GrepArgs {
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub glob: Option<String>,
    #[serde(default)]
    pub ignore_case: Option<bool>,
    #[serde(default)]
    pub literal: Option<bool>,
    #[serde(default)]
    pub context: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `bash` — one shell command with a hard timeout.
#[derive(Debug, Clone, Deserialize)]
pub struct BashArgs {
    pub command: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// `delegate_task` — hand one bounded sub-task to a child run.
///
/// No `max_depth` or budget field exists on purpose: those are runtime policy,
/// not something the model gets to negotiate per call.
///
/// There is deliberately **no `workspace` field**. A child always inherits the
/// parent turn's workspace (design §7.3): letting the model name a directory
/// would hand it a box that the sandbox then treats as "inside the workspace",
/// which would break the parent's file-access boundary.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DelegateArgs {
    pub prompt: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    /// Return as soon as the queued record and abort handle are installed.
    #[serde(default)]
    pub background: bool,
    /// Resume an existing child session instead of creating a new child.
    #[serde(default, rename = "resumeChildId", alias = "resume_child_id")]
    pub resume_child_id: Option<ChildId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_args_decode_offset_and_limit() {
        let args = ToolArgs(serde_json::json!({ "path": "a.txt", "offset": 5, "limit": 10 }));
        let parsed: ReadArgs = parse("read", &args).unwrap();
        assert_eq!(parsed.path, "a.txt");
        assert_eq!(parsed.offset, Some(5));
        assert_eq!(parsed.limit, Some(10));
    }

    #[test]
    fn missing_required_field_is_a_validation_error() {
        let args = ToolArgs(serde_json::json!({ "text": "no path" }));
        let err = parse::<ReadArgs>("read", &args).unwrap_err();
        assert!(err.message.contains("`read`"));
    }

    #[test]
    fn bash_args_keep_the_command_verbatim() {
        let args = ToolArgs(serde_json::json!({ "command": "echo \"hi\"" }));
        let parsed: BashArgs = parse("bash", &args).unwrap();
        assert_eq!(parsed.command, "echo \"hi\"");
        assert!(parsed.timeout_ms.is_none());
    }

    #[test]
    fn delegate_args_require_a_prompt_and_leave_overrides_empty() {
        let args = ToolArgs(serde_json::json!({ "prompt": "scan the repo" }));
        let parsed: DelegateArgs = parse("delegate_task", &args).unwrap();
        assert_eq!(parsed.prompt, "scan the repo");
        assert!(parsed.label.is_none());
        assert!(parsed.model.is_none());
        assert!(!parsed.background);
        assert!(parsed.resume_child_id.is_none());
    }

    #[test]
    fn delegate_args_reject_a_model_supplied_workspace() {
        // The field is gone on purpose: a child must inherit the parent's
        // workspace, so an unknown key is simply ignored rather than honoured.
        let args = ToolArgs(serde_json::json!({
            "prompt": "scan",
            "workspace": "/etc"
        }));
        let parsed: DelegateArgs = parse("delegate_task", &args).unwrap();
        assert_eq!(parsed.prompt, "scan");
    }

    #[test]
    fn delegate_args_without_a_prompt_are_a_validation_error() {
        let args = ToolArgs(serde_json::json!({ "label": "scan" }));
        let err = parse::<DelegateArgs>("delegate_task", &args).unwrap_err();
        assert!(err.message.contains("`delegate_task`"));
    }
}
