//! Tool namespace rules (design 7.5.2): `<plugin_id>.<tool>`.
//!
//! Namespacing is enforced at registration, not at call time, so a conflict is
//! a startup failure instead of a runtime surprise that silently shadows
//! another plugin's tool.

use ark_common::{ArkError, Result};

/// Longest accepted tool segment; keeps names readable in `tools/list`.
const MAX_TOOL_LEN: usize = 96;

/// Validate a plugin id: lowercase machine identity, dot-separated labels.
pub fn validate_plugin_id(plugin_id: &str) -> Result<()> {
    let ok_len = !plugin_id.is_empty() && plugin_id.len() <= 128;
    let ok_chars = plugin_id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'));
    let ok_shape =
        !plugin_id.starts_with('.') && !plugin_id.ends_with('.') && !plugin_id.contains("..");
    if ok_len && ok_chars && ok_shape {
        Ok(())
    } else {
        Err(
            ArkError::validation(format!("invalid plugin id: {plugin_id:?}"))
                .with_detail("invalid_plugin_id"),
        )
    }
}

/// Validate the local tool segment (`run_case`, `thread_list`, ...).
///
/// A dot is rejected on purpose: the published name is `<plugin_id>.<tool>`, so
/// a local part containing dots would make `owner.tool` ambiguous and could be
/// read as another plugin's namespace.
pub fn validate_tool_name(tool: &str) -> Result<()> {
    let ok_len = !tool.is_empty() && tool.len() <= MAX_TOOL_LEN;
    let ok_chars = tool
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':'));
    if ok_len && ok_chars {
        Ok(())
    } else {
        Err(ArkError::validation(format!("invalid tool name: {tool:?}"))
            .with_detail("invalid_tool_name"))
    }
}

/// Fully qualified name a tool is exposed under.
pub fn qualify(plugin_id: &str, tool: &str) -> String {
    format!("{plugin_id}.{tool}")
}

/// Reject names that try to carry a namespace of their own: a caller passing
/// `other.tool` must not be able to reach outside `plugin_id`.
pub fn ensure_local(plugin_id: &str, tool: &str) -> Result<()> {
    validate_plugin_id(plugin_id)?;
    if tool.starts_with(&format!("{plugin_id}.")) {
        return Err(ArkError::validation(format!(
            "tool name {tool:?} repeats the namespace of {plugin_id}"
        ))
        .with_detail("invalid_tool_name"));
    }
    validate_tool_name(tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_name_is_plugin_dot_tool() {
        assert_eq!(
            qualify("ark.builtin.studio", "run_case"),
            "ark.builtin.studio.run_case"
        );
    }

    #[test]
    fn plugin_id_shape_is_enforced() {
        for good in ["ark.builtin.studio", "ark.example.jira", "p1"] {
            assert!(validate_plugin_id(good).is_ok(), "{good} should be valid");
        }
        for bad in [
            "",
            "Ark.Builtin",
            "ark..builtin",
            ".ark",
            "ark/evil",
            "ark builtin",
        ] {
            assert!(validate_plugin_id(bad).is_err(), "{bad} should be invalid");
        }
    }

    #[test]
    fn tool_name_shape_is_enforced() {
        for good in ["run_case", "thread_list", "x:y", "p1"] {
            assert!(validate_tool_name(good).is_ok(), "{good} should be valid");
        }
        for bad in [
            "",
            ".hidden",
            "has space",
            "sla/sh",
            "a\nb",
            "a.b",
            "ark.b.run",
        ] {
            assert!(validate_tool_name(bad).is_err(), "{bad} should be invalid");
        }
    }

    #[test]
    fn a_tool_cannot_predeclare_its_own_namespace() {
        let err = ensure_local("ark.builtin.studio", "ark.builtin.studio.run_case").unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("invalid_tool_name"));
        assert!(ensure_local("ark.builtin.studio", "run_case").is_ok());
    }
}
