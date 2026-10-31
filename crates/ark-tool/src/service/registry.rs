//! Tool registry: registration, lookup, schema fingerprints (design §7.3).
//!
//! The registry computes each schema fingerprint **once** at registration and
//! caches it. A later `version()` bump is the signal that the prompt prefix is
//! stale, so the cache layer never has to guess whether the catalogue changed.

use ark_common::{ArkError, Result};
use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput, ToolRegistryPort};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use crate::domain::schema::fingerprint;

/// One registered tool plus its precomputed schema facts.
struct Entry {
    tool: Arc<dyn Tool>,
    schema: serde_json::Value,
    fingerprint: u64,
    group: Option<String>,
}

#[derive(Default)]
struct State {
    entries: BTreeMap<String, Entry>,
    version: u64,
}

/// Registry of tools available to one runtime.
///
/// Names are unique: a second registration of the same name is a `Conflict`, not
/// a silent override, because a silent override would change which code runs
/// behind an unchanged schema.
pub struct ToolRegistry {
    state: RwLock<State>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            state: RwLock::new(State::default()),
        }
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("names", &self.names())
            .field("version", &self.version())
            .finish()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `tool`, failing when the name is already taken.
    pub fn register(&self, tool: Arc<dyn Tool>) -> Result<()> {
        let name = tool.name().to_string();
        let mut state = self.write()?;
        if state.entries.contains_key(&name) {
            return Err(ArkError::conflict(format!(
                "tool `{name}` is already registered"
            )));
        }
        state.entries.insert(name, Entry::new(tool, None));
        state.version += 1;
        Ok(())
    }

    /// Register every tool, reporting the first conflict.
    pub fn register_all(&self, tools: Vec<Box<dyn Tool>>) -> Result<()> {
        for tool in tools {
            self.register(Arc::from(tool))?;
        }
        Ok(())
    }

    /// Replace the tools owned by one dynamic source.
    ///
    /// The replacement is atomic: callers may await while discovering remote
    /// tools, then acquire this short write lock only to publish the finished
    /// catalogue. Names owned by another source remain protected from a silent
    /// override. A schema/name change bumps the catalogue version; reconnecting
    /// an unchanged catalogue still swaps implementations so fresh sessions are
    /// used by subsequent calls.
    pub fn replace_group(&self, group: &str, tools: Vec<Arc<dyn Tool>>) -> Result<()> {
        let group = group.trim();
        if group.is_empty() {
            return Err(ArkError::validation("tool group must not be empty"));
        }
        let mut incoming = BTreeMap::new();
        for tool in tools {
            let name = tool.name().to_string();
            if incoming
                .insert(name.clone(), Entry::new(tool, Some(group.to_string())))
                .is_some()
            {
                return Err(ArkError::conflict(format!(
                    "tool `{name}` is registered more than once by group `{group}`"
                )));
            }
        }

        let mut state = self.write()?;
        for name in incoming.keys() {
            if let Some(existing) = state.entries.get(name) {
                if existing.group.as_deref() != Some(group) {
                    return Err(ArkError::conflict(format!(
                        "tool `{name}` is already registered"
                    )));
                }
            }
        }
        let changed = group_changed(&state.entries, group, &incoming);
        state
            .entries
            .retain(|_, entry| entry.group.as_deref() != Some(group));
        state.entries.extend(incoming);
        if changed {
            state.version += 1;
        }
        Ok(())
    }

    /// Look one tool up by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.read()
            .ok()?
            .entries
            .get(name)
            .map(|entry| Arc::clone(&entry.tool))
    }

    /// Whether `name` is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.read()
            .map(|state| state.entries.contains_key(name))
            .unwrap_or(false)
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
        self.read()
            .ok()?
            .entries
            .get(name)
            .map(|entry| entry.fingerprint)
    }

    /// Fingerprint over every schema in canonical order: one number that
    /// identifies the whole catalogue a prompt was built from.
    pub fn catalog_fingerprint(&self) -> u64 {
        let joined: Vec<String> = self
            .read()
            .map(|state| {
                state
                    .entries
                    .values()
                    .map(|entry| crate::domain::schema::canonical(&entry.schema))
                    .collect()
            })
            .unwrap_or_default();
        crate::domain::schema::fingerprint_text(&joined.join("\u{1f}"))
    }

    /// Read-only snapshot for the agent, which only needs names and schemas.
    pub fn port(self: &Arc<Self>) -> Arc<dyn ToolRegistryPort> {
        Arc::clone(self) as Arc<dyn ToolRegistryPort>
    }

    fn read(&self) -> Result<std::sync::RwLockReadGuard<'_, State>> {
        self.state
            .read()
            .map_err(|_| ArkError::internal("tool registry read lock poisoned"))
    }

    fn write(&self) -> Result<std::sync::RwLockWriteGuard<'_, State>> {
        self.state
            .write()
            .map_err(|_| ArkError::internal("tool registry write lock poisoned"))
    }
}

impl ToolRegistryPort for ToolRegistry {
    fn names(&self) -> Vec<String> {
        self.read()
            .map(|state| state.entries.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn schemas(&self) -> Vec<serde_json::Value> {
        self.read()
            .map(|state| {
                state
                    .entries
                    .values()
                    .map(|entry| {
                        // Uniform function shape `{name, description?,
                        // parameters}`: OpenAI chat wraps this under
                        // `function` verbatim, while the Responses and
                        // Anthropic adapters pick `name`/`description`/
                        // `parameters` out of it, so no adapter needs a
                        // per-tool branch. `description` is only emitted
                        // when the tool schema declares one.
                        let mut value = serde_json::json!({
                            "name": entry.tool.name(),
                            "parameters": entry.schema,
                        });
                        if let Some(description) = entry.schema.get("description") {
                            value["description"] = description.clone();
                        }
                        value
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn version(&self) -> u64 {
        self.read().map(|state| state.version).unwrap_or_default()
    }
}

impl Entry {
    fn new(tool: Arc<dyn Tool>, group: Option<String>) -> Self {
        let schema = tool.schema().clone();
        let fingerprint = fingerprint(&schema);
        Self {
            tool,
            schema,
            fingerprint,
            group,
        }
    }
}

fn group_changed(
    entries: &BTreeMap<String, Entry>,
    group: &str,
    incoming: &BTreeMap<String, Entry>,
) -> bool {
    let existing: BTreeMap<&str, u64> = entries
        .iter()
        .filter_map(|(name, entry)| {
            (entry.group.as_deref() == Some(group)).then_some((name.as_str(), entry.fingerprint))
        })
        .collect();
    let replacement: BTreeMap<&str, u64> = incoming
        .iter()
        .map(|(name, entry)| (name.as_str(), entry.fingerprint))
        .collect();
    existing != replacement
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::tools::builtins;
    use ark_common::{ThreadId, TurnId};
    use ark_ports::{SandboxDecision, Tool};
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    struct NamedTool {
        name: &'static str,
        schema: &'static serde_json::Value,
    }

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &'static str {
            self.name
        }

        fn schema(&self) -> &'static serde_json::Value {
            self.schema
        }

        async fn execute(&self, _ctx: &ToolContext, _args: ToolArgs) -> Result<ToolOutput> {
            Ok(ToolOutput::ok("ok"))
        }
    }

    fn named_tool(name: &'static str) -> Arc<dyn Tool> {
        Arc::new(NamedTool {
            name,
            schema: Box::leak(Box::new(serde_json::json!({ "type": "object" }))),
        })
    }

    fn registry() -> ToolRegistry {
        let registry = ToolRegistry::new();
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
        let registry = ToolRegistry::new();
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

    #[test]
    fn schemas_use_the_uniform_function_shape() {
        let registry = registry();
        let bash = registry
            .schemas()
            .into_iter()
            .find(|schema| schema["name"] == "bash")
            .expect("bash schema");
        // Function shape: the raw JSON Schema lives under `parameters`, never
        // bare at the top level.
        assert_eq!(bash["parameters"]["type"], "object");
        assert!(bash["parameters"]["properties"]["command"].is_object());
        assert!(bash["parameters"]["required"].is_array());
        assert!(
            bash.get("type").is_none(),
            "schema must not leak to top level"
        );
        assert!(bash.get("properties").is_none());
        // `description` is only emitted when the tool schema declares one;
        // bash declares none, so the key must be absent rather than faked.
        assert!(bash.get("description").is_none());
    }

    #[test]
    fn replacing_a_group_refreshes_catalog_without_duplicates_and_bumps_version() {
        let registry = ToolRegistry::new();
        registry.register(named_tool("read")).unwrap();
        let initial_version = registry.version();

        registry
            .replace_group("mcp", vec![named_tool("probe.ping")])
            .unwrap();
        assert_eq!(registry.names(), vec!["probe.ping", "read"]);
        assert_eq!(registry.version(), initial_version + 1);

        registry
            .replace_group("mcp", vec![named_tool("probe.ping")])
            .unwrap();
        assert_eq!(registry.names(), vec!["probe.ping", "read"]);
        assert_eq!(registry.version(), initial_version + 1);

        registry.replace_group("mcp", Vec::new()).unwrap();
        assert_eq!(registry.names(), vec!["read"]);
        assert_eq!(registry.version(), initial_version + 2);
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
