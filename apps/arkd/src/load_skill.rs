//! The model-facing `load_skill` tool belongs in the composition root: it uses
//! the concrete skill runtime without making `ark-tool` depend on `ark-skills`.

use std::sync::{Arc, LazyLock};

use ark_common::Result;
use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

static LOAD_SKILL_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "description": "Load the full instructions for one available skill.",
        "properties": {
            "skill_id": {
                "type": "string",
                "description": "The skill id from the skills catalog."
            }
        },
        "required": ["skill_id"],
        "additionalProperties": false
    })
});

/// Loads a discovered skill's instructions for the model.
pub struct LoadSkillTool {
    skills: Arc<ark_skills::SkillRuntime>,
}

impl LoadSkillTool {
    /// Create a tool backed by the process-wide skill snapshot.
    pub fn new(skills: Arc<ark_skills::SkillRuntime>) -> Self {
        Self { skills }
    }
}

#[async_trait]
impl Tool for LoadSkillTool {
    fn name(&self) -> &'static str {
        "load_skill"
    }

    fn schema(&self) -> &'static Value {
        &LOAD_SKILL_SCHEMA
    }

    async fn execute(&self, _ctx: &ToolContext, args: ToolArgs) -> Result<ToolOutput> {
        let Some(skill_id) = args
            .get_str("skill_id")
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            return Ok(ToolOutput::err(
                "load_skill requires a non-empty `skill_id`",
            ));
        };
        match self.skills.instruction(skill_id) {
            Some(body) => Ok(ToolOutput::ok(body)),
            None => {
                let available = self
                    .skills
                    .summaries()
                    .into_iter()
                    .map(|skill| skill.id)
                    .collect::<Vec<_>>();
                let listed = if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                };
                Ok(ToolOutput::err(format!(
                    "skill `{skill_id}` is unavailable; available ids: {listed}"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ark_ports::{SandboxDecision, Tool, ToolArgs, ToolContext};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::LoadSkillTool;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("arkd-load-skill-{}", ark_common::TurnId::new()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn runtime_with_skill() -> Arc<ark_skills::SkillRuntime> {
        let dir = TestDir::new();
        let skill = dir.0.join("inspect");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("skill.json"),
            r#"{"id":"inspect","name":"Inspect"}"#,
        )
        .unwrap();
        fs::write(skill.join("SKILL.md"), "Read the repository first.").unwrap();
        let config = ark_skills::SkillsConfig {
            roots: vec![dir.0.clone()],
            workspace_dirs: Vec::new(),
            ..ark_skills::SkillsConfig::default()
        };
        Arc::new(ark_skills::SkillRuntime::new(config, dir.0.clone()))
    }

    fn context() -> ToolContext {
        ToolContext {
            thread_id: ark_common::ThreadId::new(),
            turn_id: ark_common::TurnId::new(),
            workspace: std::env::temp_dir(),
            sandbox: SandboxDecision::ReadOnly,
            env: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn load_skill_returns_the_instruction_body_for_a_known_id() {
        let tool = LoadSkillTool::new(runtime_with_skill());

        let output = tool
            .execute(
                &context(),
                ToolArgs(serde_json::json!({ "skill_id": "inspect" })),
            )
            .await
            .unwrap();

        assert!(output.ok);
        assert_eq!(output.content, "Read the repository first.");
    }

    #[tokio::test]
    async fn load_skill_lists_available_ids_when_the_requested_id_is_missing() {
        let tool = LoadSkillTool::new(runtime_with_skill());

        let output = tool
            .execute(
                &context(),
                ToolArgs(serde_json::json!({ "skill_id": "missing" })),
            )
            .await
            .unwrap();

        assert!(!output.ok);
        assert!(output.content.contains("inspect"), "{}", output.content);
    }
}
