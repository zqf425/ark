//! Pieces both turn paths (parent `run_turn` and child `run_child_turn`)
//! share. Each one exists because the two paths must not drift: the prompt-cache
//! invariant ("the static prefix is byte-identical across parent and child"),
//! the per-turn limits and the hook-wrapped tool execution are one behavior,
//! not two.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::history::workspace_line;
use super::*;

/// The static prompt-cache prefix for every turn: system prompt plus the
/// workspace line. Parent and child must emit these exact bytes first, or the
/// provider caches two prefixes and the shared cache is lost.
pub(crate) fn seed_prefix(workspace: &Path) -> Vec<ModelMessage> {
    vec![
        ModelMessage::system(SYSTEM_PROMPT),
        ModelMessage::system(workspace_line(workspace)),
    ]
}

/// Skill instructions (and optionally the catalog line) as the dynamic tail.
///
/// Only activated skills are injected; the rest appear in the catalog line so
/// the model knows they exist (design §7.4.6 / §7.14).
pub(crate) fn skill_tail(
    resolution: &ark_contract::SkillTurnResolution,
    include_catalog: bool,
) -> Vec<ModelMessage> {
    let mut messages: Vec<ModelMessage> = resolution
        .instructions
        .iter()
        .map(|instruction| ModelMessage::system(format!("[skill instructions]\n{instruction}")))
        .collect();
    if include_catalog {
        if let Some(catalog) = &resolution.catalog {
            messages.push(ModelMessage::system(format!(
                "[available skills]\n{catalog}"
            )));
        }
    }
    messages
}

/// Canonicalize an explicit or inherited workspace override, or fall back.
///
/// `explicit` (a per-session override) wins over `inherited` (the parent's
/// effective workspace); `fallback` is the process lease/current root. A
/// `None` explicit value is intentionally not a reset: only an explicit value
/// changes inherited state. The `fallback` path is validated too: a lease root
/// that vanished mid-session fails the turn up front instead of failing every
/// tool call later.
pub(crate) fn resolve_workspace(
    explicit: Option<&Path>,
    inherited: Option<PathBuf>,
    fallback: PathBuf,
    label: &str,
) -> Result<PathBuf> {
    let workspace = explicit
        .map(Path::to_path_buf)
        .or(inherited)
        .unwrap_or(fallback);
    if !workspace.is_dir() {
        return Err(ark_common::ArkError::validation(format!(
            "{label} workspace does not exist: {}",
            workspace.display()
        )));
    }
    workspace
        .canonicalize()
        .map(|path| {
            std::path::PathBuf::from(ark_common::path::strip_verbatim(&path.to_string_lossy()))
        })
        .map_err(|_| {
            ark_common::ArkError::validation(format!(
                "cannot canonicalize {label} workspace: {}",
                workspace.display()
            ))
        })
}

/// Per-turn limits from the live `tuning` block.
///
/// A child gets the same ceilings as a parent, never more; a running turn
/// keeps the snapshot it started with and the next turn sees the edit.
pub(crate) fn limits_for_turn(
    snapshot: &RuntimeConfig,
    provider: Option<&ark_llm::ProviderConfig>,
) -> TurnLimits {
    let tuning = &snapshot.tuning;
    TurnLimits {
        max_rounds: tuning.max_rounds.max(1),
        max_tokens: tuning.max_tokens.max(1),
        max_tool_calls: tuning.max_tool_calls.max(1),
        max_identical_calls: tuning.max_identical_calls.max(1),
        context_limit_tokens: model::context_limit_tokens(tuning.context_limit_tokens, provider),
        keep_recent_messages: tuning.keep_recent_messages.max(1),
        max_compactions: TurnLimits::default().max_compactions,
    }
}

/// Model-delta channel capacity from `tuning`, falling back to the default.
pub(crate) fn chunk_buffer_for_turn(snapshot: &RuntimeConfig) -> usize {
    let tuned = snapshot.tuning.chunk_buffer;
    if tuned == 0 {
        CHUNK_BUFFER
    } else {
        tuned
    }
}

/// The one tool runner both paths use: `ark-tool`'s registry runner wrapped in
/// the pre-tool-use hook combinator.
///
/// The reactor already decided the sandbox mode for a call and carries it in
/// `call.sandbox`; re-deciding would flip an approved `bash` back to
/// `NeedsApproval` (design §7.3, P1-5). `run_in` remains the fallback for
/// calls that never went through the model path.
pub(crate) struct HookedToolRunner {
    inner: Arc<RegistryToolRunner>,
    /// Live config: hooks are read per call so a patch applies immediately.
    config: Arc<ConfigStore>,
    workspace: PathBuf,
    thread_id: ThreadId,
    turn_id: TurnId,
}

impl HookedToolRunner {
    pub(crate) fn new(
        inner: Arc<RegistryToolRunner>,
        config: Arc<ConfigStore>,
        workspace: PathBuf,
        thread_id: ThreadId,
        turn_id: TurnId,
    ) -> Self {
        Self {
            inner,
            config,
            workspace,
            thread_id,
            turn_id,
        }
    }
}

#[async_trait::async_trait]
impl AgentToolRunner for HookedToolRunner {
    async fn execute(&self, call: &ToolCallRequest) -> Result<ToolOutput> {
        let snapshot = self.config.snapshot();
        let hooks = &snapshot.hooks;
        let payload = crate::hooks::hook_payload(
            "pre_tool_use",
            &call.tool,
            &call.arguments,
            &self.thread_id,
            &self.turn_id,
        );
        crate::hooks::run_around(hooks, &self.workspace, &payload, || async {
            match call.sandbox {
                Some(decision) => {
                    self.inner
                        .run_decided(
                            self.workspace.clone(),
                            self.thread_id,
                            self.turn_id,
                            &call.tool,
                            call.arguments.clone(),
                            decision,
                        )
                        .await
                }
                None => {
                    self.inner
                        .run_in(
                            self.workspace.clone(),
                            self.thread_id,
                            self.turn_id,
                            &call.tool,
                            call.arguments.clone(),
                        )
                        .await
                }
            }
        })
        .await
    }
}

/// Everything one reactor instance needs, assembled by either turn path.
///
/// Both paths build the same shape; collecting it here keeps the construction
/// (and the field-by-field mapping) from drifting between parent and child.
pub(crate) struct ReactorSpec {
    pub model: Arc<dyn ModelClient>,
    pub tools: Arc<dyn AgentToolRunner>,
    pub writer: Arc<EventSinkWriter>,
    pub gates: Arc<dyn GateBroker>,
    pub sandbox: Arc<dyn SandboxPolicy>,
    pub events: TurnEvents,
    pub cancel: CancelToken,
    pub seed: ModelSeed,
    pub machine: TurnMachine,
    pub chunk_buffer: usize,
    pub steering: Arc<SteerQueue>,
}

pub(crate) fn build_reactor(spec: ReactorSpec) -> Reactor {
    Reactor::new(ReactorConfig {
        model: spec.model,
        tools: spec.tools,
        writer: spec.writer,
        gates: spec.gates,
        sandbox: spec.sandbox,
        events: spec.events,
        cancel: spec.cancel,
        seed: spec.seed,
        machine: spec.machine,
        chunk_buffer: spec.chunk_buffer,
    })
    .with_steering(spec.steering)
}

/// Who the child is, appended to the shared prefix before the skill tail.
pub(crate) const CHILD_ROLE_LINE: &str =
    "You are a child run. Finish the delegated task and answer with the result. \
     You cannot delegate further.";

#[cfg(test)]
mod tests {
    use super::*;

    /// The prompt-cache invariant: the child's static prefix must be the
    /// parent's static prefix, byte for byte. Both paths build theirs from
    /// [`seed_prefix`]; this test composes both the way the two call sites do
    /// and asserts the shared leading window. If a path stops routing its
    /// prefix through the shared builder, update this test *and* ask why the
    /// builder stopped being enough.
    #[test]
    fn the_child_prefix_extends_the_parent_prefix_byte_for_byte() {
        let workspace = std::path::Path::new("/tmp/ark");
        // Parent path: seed_prefix, then history and the dynamic tail.
        let mut parent = seed_prefix(workspace);
        parent.push(ModelMessage::system("history message one"));
        parent.push(ModelMessage::system("[skill instructions]\nfallback"));
        // Child path: seed_prefix, then the role line and the skill tail.
        let mut child = seed_prefix(workspace);
        child.push(ModelMessage::system(CHILD_ROLE_LINE));
        child.push(ModelMessage::system("[skill instructions]\nfallback"));

        assert_eq!(
            child[..2],
            parent[..2],
            "the shared prompt-cache window diverged"
        );
        // The first divergence is exactly the role line vs the first history
        // message: nothing else may differ before the dynamic tail.
        assert_ne!(child[2].content, parent[2].content);
        assert_eq!(child[2].content, CHILD_ROLE_LINE);
    }
}
