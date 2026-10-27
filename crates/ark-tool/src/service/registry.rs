//! Tool registry: registration, lookup, schema fingerprints (design §7.3).
//!
//! The registry computes each schema fingerprint **once** at registration and
//! caches it. A later `version()` bump is the signal that the prompt prefix is
//! stale, so the cache layer never has to guess whether the catalogue changed.

use ark_common::{ArkError, Result};
use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput, ToolRegistryPort};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::schema::fingerprint;

/// One registered tool plus its precomputed schema facts.
struct Entry {
    tool: Arc<dyn Tool>,
    schema: serde_json::Value,
    fingerprint: u64,
}

/// Registry of tools available to one runtime.
///
/// Names are unique: a second registration of the same name is a `Conflict`, not
/// a silent override, because a silent override would change which code runs
/// behind an unchanged schema.
#[derive(Default)]
pub struct ToolRegistry {
    entries: BTreeMap<String, Entry>,
    version: u64,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("names", &self.names())
            .field("version", &self.version)
            .finish()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `tool`, failing when the name is already taken.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        let name = tool.name().to_string();
        if self.entries.contains_key(&name) {
            return Err(ArkError::conflict(format!(
                "tool `{name}` is already registered"
            )));
        }
        let schema = tool.schema().clone();
        let fingerprint = fingerprint(&schema);
        self.entries.insert(
            name,
            Entry {
                tool,
                schema,
                fingerprint,
            },
        );
        self.version += 1;
        Ok(())
    }

    /// Register every tool, reporting the first conflict.
    pub fn register_all(&mut self, tools: Vec<Box<dyn Tool>>) -> Result<()> {
        for tool in tools {
            self.register(Arc::from(tool))?;
        }
        Ok(())
    }

    /// Look one tool up by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.entries.get(name).map(|entry| Arc::clone(&entry.tool))
    }

    /// Whether `name` is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Execute one call against the registry.
    ///
    /// An unknown tool is a `NotFound` error rather than a panic or a synthetic
    /// success, so the model sees an honest answer.
    pub async fn execute(
        &self,
        name: &str,
        ctx: &ToolContext,
        args: ToolArgs,
    ) -> Result<ToolOutput> {
        let tool = self
            .get(name)
            .ok_or_else(|| ArkError::not_found(format!("tool `{name}` is not registered")))?;
        tool.execute(ctx, args).await
    }

    /// Cached fingerprint of one tool schema.
    pub fn fingerprint_of(&self, name: &str) -> Option<u64> {
        self.entries.get(name).map(|entry| entry.fingerprint)
    }

    /// Fingerprint over every schema in canonical order: one number that
    /// identifies the whole catalogue a prompt was built from.
    pub fn catalog_fingerprint(&self) -> u64 {
        let joined: Vec<String> = self
            .entries
            .values()
            .map(|entry| crate::domain::schema::canonical(&entry.schema))
            .collect();
        crate::domain::schema::fingerprint_text(&joined.join("\u{1f}"))
    }

    /// Read-only snapshot for the agent, which only needs names and schemas.
    pub fn port(self: &Arc<Self>) -> Arc<dyn ToolRegistryPort> {
        Arc::clone(self) as Arc<dyn ToolRegistryPort>
    }
}

impl ToolRegistryPort for ToolRegistry {
    fn names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    fn schemas(&self) -> Vec<serde_json::Value> {
        self.entries
            .values()
            .map(|entry| {
                // The schema already carries `name`/`description` only where the
                // tool declared them; expose a uniform function shape so a
                // provider adapter needs no per-tool branch.
                let mut value = entry.schema.clone();
                if let Some(object) = value.as_object_mut() {
                    object
                        .entry("name")
                        .or_insert_with(|| entry.tool.name().into());
                }
                value
            })
            .collect()
    }

    fn version(&self) -> u64 {
        self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::tools::builtins;
    use ark_common::{ThreadId, TurnId};
    use ark_ports::SandboxDecision;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register_all(builtins()).unwrap();
        registry
    }

    fn ctx() -> ToolContext {
        ToolContext {
            thread_id: ThreadId::new(),
            turn_id: TurnId::new(),
            workspace: std::env::temp_dir(),
            sandbox: SandboxDecision::ReadOnly,
            env: BTreeMap::<String, String>::new(),
        }
    }

    #[test]
    fn builtins_register_in_canonical_order() {
        let registry = registry();
        assert_eq!(
            registry.names(),
            vec!["bash", "edit", "find", "grep", "ls", "read", "write"]
        );
        assert_eq!(registry.version(), 7);
    }

    #[test]
    fn duplicate_registration_conflicts() {
        let mut registry = ToolRegistry::new();
        registry.register_all(builtins()).unwrap();
        let err = registry.register_all(builtins()).unwrap_err();
        assert!(err.message.contains("already registered"));
        assert_eq!(registry.version(), 7, "a failed register does not bump");
    }

    #[test]
    fn schemas_are_stable_and_fingerprinted() {
        let registry = registry();
        let first = registry.catalog_fingerprint();
        assert_eq!(first, registry.catalog_fingerprint());
        assert!(registry.fingerprint_of("read").is_some());
        assert_eq!(registry.fingerprint_of("nope"), None);
    }

    #[test]
    fn schemas_carry_the_tool_name() {
        let registry = registry();
        let schemas = registry.schemas();
        let names: Vec<String> = schemas
            .iter()
            .filter_map(|schema| schema["name"].as_str().map(str::to_string))
            .collect();
        assert_eq!(names.len(), 7);
        assert!(names.contains(&"read".to_string()));
    }

    #[tokio::test]
    async fn unknown_tool_is_not_found() {
        let registry = registry();
        let err = registry
            .execute("nope", &ctx(), ToolArgs::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
    }

    #[tokio::test]
    async fn registered_tool_runs_through_the_registry() {
        let registry = registry();
        let dir = crate::exec::testsupport::temp_workspace("registry-exec");
        std::fs::write(dir.join("a.txt"), "body\n").unwrap();
        let mut context = ctx();
        context.workspace = PathBuf::from(&dir);
        let out = registry
            .execute(
                "read",
                &context,
                ToolArgs(serde_json::json!({ "path": "a.txt" })),
            )
            .await
            .unwrap();
        assert!(out.ok);
        assert!(out.content.contains("body"));
    }

    #[test]
    fn port_view_exposes_the_same_catalogue() {
        let registry = Arc::new(registry());
        let port = registry.port();
        assert_eq!(port.names().len(), 7);
        assert_eq!(port.version(), 7);
        assert_eq!(port.schemas().len(), 7);
    }
}
