//! Explicit visibility between plugins (design 7.5.2).
//!
//! A manifest declares `mcp.expose` (what it publishes) and `mcp.consume`
//! (which other plugins' tools it may call). A call that was not declared is
//! rejected, so the dependency graph is auditable and can be topologically
//! sorted before startup.
//!
//! # Why allow entries are resolved at match time
//!
//! Both plugin ids and tool names may contain dots (`ark.example.jira`), so
//! `ark.example.jira.search` cannot be split into "provider + tool" without
//! knowing the registered provider ids. Entries are therefore kept verbatim and
//! matched against the *actual* provider id and qualified tool name:
//!
//! * `ark.example.jira` matches provider `ark.example.jira`, any tool;
//! * `ark.example.jira.search` matches provider `ark.example.jira`, tool
//!   `search` only.
//!
//! That removes the ambiguity instead of guessing at parse time.

use ark_common::{ArkError, Result};
use std::collections::{BTreeMap, BTreeSet};

use crate::domain::namespace::{qualify, validate_plugin_id, validate_tool_name};

/// One `mcp.consume` entry. Kept as written; interpretation happens in
/// [`McpVisibility::may_call`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VisibilityScope(String);

impl VisibilityScope {
    /// Accept only provider-shaped or `provider.tool`-shaped entries.
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(ArkError::validation("empty mcp visibility entry"));
        }
        if raw.len() > 200 {
            return Err(ArkError::validation("mcp visibility entry is too long"));
        }
        let ok_chars = raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'));
        let ok_shape = !raw.starts_with('.') && !raw.ends_with('.') && !raw.contains("..");
        if !ok_chars || !ok_shape {
            return Err(ArkError::validation(format!(
                "invalid mcp visibility entry {raw:?}"
            )));
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Entry equals the provider id verbatim.
    pub fn covers_provider(&self, provider: &str) -> bool {
        self.0 == provider
    }

    /// Entry equals the qualified tool name.
    pub fn covers_tool(&self, provider: &str, tool: &str) -> bool {
        self.0 == qualify(provider, tool)
    }

    /// Entry compared without knowing which provider ids exist. Callers that do
    /// know them should go through [`McpVisibility::may_call`], which resolves
    /// `owner.tool` against the declared providers first.
    pub fn covers(&self, provider: &str, tool: &str) -> bool {
        self.covers_provider(provider) || self.covers_tool(provider, tool)
    }
}

/// Declared exposure and consumption for every plugin.
#[derive(Debug, Default)]
pub struct McpVisibility {
    /// provider -> tools it publishes to the outside.
    exposed: BTreeMap<String, BTreeSet<String>>,
    /// caller -> what it may call.
    consumed: BTreeMap<String, BTreeSet<VisibilityScope>>,
}

impl McpVisibility {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one plugin's declaration. Re-declaring replaces, which is what a
    /// reload needs; a plugin that declares nothing consumes nothing.
    pub fn declare(
        &mut self,
        plugin_id: &str,
        expose: &[String],
        consume: &[String],
    ) -> Result<()> {
        validate_plugin_id(plugin_id)?;
        let mut tools = BTreeSet::new();
        for tool in expose {
            validate_tool_name(tool)?;
            tools.insert(tool.clone());
        }
        let mut scopes = BTreeSet::new();
        for entry in consume {
            scopes.insert(VisibilityScope::parse(entry)?);
        }
        self.exposed.insert(plugin_id.to_string(), tools);
        self.consumed.insert(plugin_id.to_string(), scopes);
        Ok(())
    }

    pub fn forget(&mut self, plugin_id: &str) {
        self.exposed.remove(plugin_id);
        self.consumed.remove(plugin_id);
    }

    pub fn exposes(&self, provider: &str, tool: &str) -> bool {
        self.exposed
            .get(provider)
            .map(|tools| tools.contains(tool))
            .unwrap_or(false)
    }

    /// Does `caller` declare `provider.tool`?
    pub fn may_call(&self, caller: &str, provider: &str, tool: &str) -> bool {
        if caller == provider {
            // Intra-plugin calls need no declaration: a plugin always reaches
            // its own tools, and it cannot impersonate another plugin id.
            return true;
        }
        self.consumed
            .get(caller)
            .map(|scopes| {
                scopes
                    .iter()
                    .any(|scope| self.scope_covers(scope, provider, tool))
            })
            .unwrap_or(false)
    }

    /// Interpret one entry against the providers that actually declared.
    ///
    /// `ark.example.jira.search` is a tool scope only when `ark.example.jira` is
    /// a declared provider; otherwise the whole string is a provider id. Without
    /// this split a tool-scoped entry would also grant a provider whose id
    /// happens to equal the entry, which would widen the permission.
    fn scope_covers(&self, scope: &VisibilityScope, provider: &str, tool: &str) -> bool {
        let entry = scope.as_str();
        if let Some((prefix, name)) = entry.rsplit_once('.') {
            if self.exposed.contains_key(prefix) {
                return prefix == provider && name == tool;
            }
        }
        entry == provider
    }

    /// Fail-closed gate used on the call path.
    pub fn authorize(&self, caller: &str, provider: &str, tool: &str) -> Result<()> {
        validate_plugin_id(caller)?;
        validate_plugin_id(provider)?;
        validate_tool_name(tool)?;
        if !self.may_call(caller, provider, tool) {
            return Err(ArkError::new(
                ark_common::ArkErrorCode::Unauthorized,
                format!(
                    "{caller} did not declare mcp.consume for {}",
                    qualify(provider, tool)
                ),
            )
            .with_detail("mcp_not_permitted"));
        }
        Ok(())
    }

    /// Provider ids `caller` is allowed to reach (used by catalog filtering).
    ///
    /// A tool-scoped entry contributes its provider, so filtering stays
    /// permissive at the catalog level while [`Self::authorize`] keeps the
    /// per-tool check exact.
    pub fn allowed_providers(&self, caller: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let Some(scopes) = self.consumed.get(caller) else {
            out.insert(caller.to_string());
            return out;
        };
        out.insert(caller.to_string());
        for scope in scopes {
            out.insert(scope.as_str().to_string());
            // A tool-scoped entry also names its provider as a prefix.
            let mut candidate = scope.as_str();
            while let Some(idx) = candidate.rfind('.') {
                candidate = &candidate[..idx];
                out.insert(candidate.to_string());
            }
        }
        out
    }

    /// Exact per-tool check used when filtering a catalog for one caller.
    pub fn permits_tool(&self, caller: &str, provider: &str, tool: &str) -> bool {
        self.may_call(caller, provider, tool)
    }
}

/// Convenience wrapper for manifest structs that carry the two lists.
pub fn declare(
    visibility: &mut McpVisibility,
    plugin_id: &str,
    expose: &[String],
    consume: &[String],
) -> Result<()> {
    visibility.declare(plugin_id, expose, consume)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_entry_covers_every_tool() {
        let mut visibility = McpVisibility::new();
        visibility
            .declare(
                "ark.example.jira",
                &["search".into()],
                &["ark.example.jira".into()],
            )
            .unwrap();
        assert!(visibility.may_call("ark.example.jira", "ark.example.jira", "search"));
        assert!(!visibility.may_call("ark.other", "ark.example.jira2", "search"));
    }

    #[test]
    fn tool_entry_is_exact() {
        let mut visibility = McpVisibility::new();
        // The provider is declared, so `ark.example.jira.search` is a tool scope
        // and must not be read as a provider id of the same spelling.
        visibility
            .declare("ark.example.jira", &["search".into()], &[])
            .unwrap();
        visibility
            .declare("ark.example.jira.search", &["go".into()], &[])
            .unwrap();
        visibility
            .declare(
                "ark.builtin.studio",
                &[],
                &["ark.example.jira.search".into()],
            )
            .unwrap();
        assert!(visibility.may_call("ark.builtin.studio", "ark.example.jira", "search"));
        assert!(!visibility.may_call("ark.builtin.studio", "ark.example.jira", "delete_issue"));
        assert!(!visibility.may_call("ark.builtin.studio", "ark.example.jira.search", "go"));
    }

    #[test]
    fn undeclared_cross_plugin_call_is_denied() {
        let mut visibility = McpVisibility::new();
        visibility
            .declare("ark.builtin.studio", &["run_case".into()], &[])
            .unwrap();
        let err = visibility
            .authorize("ark.builtin.studio", "ark.example.jira", "search")
            .unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("mcp_not_permitted"));
    }

    #[test]
    fn declared_provider_call_is_allowed() {
        let mut visibility = McpVisibility::new();
        visibility
            .declare("ark.builtin.studio", &[], &["ark.example.jira".into()])
            .unwrap();
        assert!(visibility
            .authorize("ark.builtin.studio", "ark.example.jira", "search")
            .is_ok());
        assert!(visibility
            .authorize("ark.builtin.studio", "ark.example.jira", "delete_issue")
            .is_ok());
    }

    #[test]
    fn tool_scoped_declaration_is_narrow() {
        let mut visibility = McpVisibility::new();
        visibility
            .declare("ark.example.jira", &["search".into()], &[])
            .unwrap();
        visibility
            .declare(
                "ark.builtin.studio",
                &[],
                &["ark.example.jira.search".into()],
            )
            .unwrap();
        assert!(visibility
            .authorize("ark.builtin.studio", "ark.example.jira", "search")
            .is_ok());
        assert!(visibility
            .authorize("ark.builtin.studio", "ark.example.jira", "delete_issue")
            .is_err());
    }

    #[test]
    fn a_plugin_reaches_its_own_tools_without_declaration() {
        let visibility = McpVisibility::new();
        assert!(visibility
            .authorize("ark.example.jira", "ark.example.jira", "search")
            .is_ok());
    }

    #[test]
    fn expose_list_is_recorded_per_provider() {
        let mut visibility = McpVisibility::new();
        visibility
            .declare("ark.example.jira", &["search".into()], &[])
            .unwrap();
        assert!(visibility.exposes("ark.example.jira", "search"));
        assert!(!visibility.exposes("ark.example.jira", "delete_issue"));
    }

    #[test]
    fn invalid_declarations_are_rejected() {
        let mut visibility = McpVisibility::new();
        assert!(visibility.declare("Ark.Bad", &[], &[]).is_err());
        assert!(visibility
            .declare("ark.a", &["bad name".into()], &[])
            .is_err());
        assert!(visibility.declare("ark.a", &[], &["".into()]).is_err());
    }
}
