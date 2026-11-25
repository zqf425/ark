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

/// Parent-only policy for explicit interactive input.
///
/// It belongs to the dynamic tail: putting it in the shared prefix would change
/// child prompt-cache bytes and tell a child to use a tool it does not have.
/// Headless and system-origin turns stay out so shutdown cannot orphan a child
/// and completion notifications cannot trigger another delegation loop.
pub(crate) fn delegation_policy_message(
    unattended: bool,
    source: &TurnSource,
    settings: &ark_contract::SubagentSettings,
) -> Option<ModelMessage> {
    if unattended || !settings.enabled || !settings.default_background {
        return None;
    }
    if !matches!(
        source,
        TurnSource::User(_) | TurnSource::Im { .. } | TurnSource::AutoRecovery { .. }
    ) {
        return None;
    }
    Some(ModelMessage::system(
        "[delegation policy]\n\
         For substantive coding, investigation, or multi-step execution, delegate the work \
         to background subagents by default. Call `delegate_task` without a `background` \
         override (or with `background: true`), and split independent work into parallel \
         child tasks. Keep the main session responsible for planning, user communication, \
         and final synthesis; acknowledge delegated work briefly and do not repeat it. \
         Handle simple conversation, clarification, and trivial lookups directly. Use \
         foreground delegation only when the user explicitly requires a blocking result. \
         Treat child completion notifications as results to synthesize, not as new tasks \
         to delegate again.",
    ))
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
    // `0` is the settings UI's "unlimited" value for the two turn budgets.
    // Translate it once here into the largest finite limit: the agent's
    // budget policy keeps its plain `counter >= limit` semantics, and a real
    // turn counter can never reach `u32::MAX` / `u64::MAX`.
    TurnLimits {
        max_rounds: if tuning.max_rounds == 0 {
            u32::MAX
        } else {
            tuning.max_rounds
        },
        max_tokens: if tuning.max_tokens == 0 {
            u64::MAX
        } else {
            tuning.max_tokens
        },
        max_tool_calls: tuning.max_tool_calls.max(1),
        max_identical_calls: tuning.max_identical_calls.max(1),
        context_limit_tokens: model::context_limit_tokens(provider),
        keep_recent_messages: tuning.keep_recent_messages.max(1),
        max_compactions: TurnLimits::default().max_compactions,
        tool_storm_window_size: tuning.tool_storm_window_size,
        max_parallel_tool_calls: tuning.max_parallel_tool_calls,
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

/// Plugin-hook decorator around [`HookedToolRunner`] (requirements/
/// plugin-hook-bus.md, P1).
///
/// The order is pinned and tested: plugin decide first, then the shell
/// `pre_tool_use` hook, then dispatch. With [`ark_ports::NoHooks`] installed
/// this layer forwards the call unchanged, so the shell-hook semantics the
/// runtime ships today are untouched.
pub(crate) struct HookToolRunner {
    hooks: Arc<dyn ark_ports::HookBroker>,
    thread_id: ThreadId,
    turn_id: TurnId,
    inner: HookedToolRunner,
}

impl HookToolRunner {
    pub(crate) fn new(
        hooks: Arc<dyn ark_ports::HookBroker>,
        thread_id: ThreadId,
        turn_id: TurnId,
        inner: HookedToolRunner,
    ) -> Self {
        Self {
            hooks,
            thread_id,
            turn_id,
            inner,
        }
    }
}

/// Serialized argument head for the hook payload, capped at the design budget.
///
/// Arguments stay opaque JSON to the hook: a plugin that wants a path reads it
/// from the head, never from a parsed copy the host would have to trust.
fn args_head(arguments: &serde_json::Value) -> (String, bool) {
    let serialized = arguments.to_string();
    let (head, truncated) = ark_ports::truncate_utf8(&serialized, ark_ports::TOOL_ARGS_HEAD_BYTES);
    (head.to_string(), truncated)
}

#[async_trait::async_trait]
impl AgentToolRunner for HookToolRunner {
    async fn execute(&self, call: &ToolCallRequest) -> Result<ToolOutput> {
        let scope = ark_ports::HookScope {
            thread_id: &self.thread_id,
            turn_id: &self.turn_id,
        };
        let (args_head, args_truncated) = args_head(&call.arguments);
        match self
            .hooks
            .on_tool_pre_call(scope, &call.tool, &args_head, args_truncated)
            .await?
        {
            ark_ports::ToolPreCallVerdict::Allow => self.inner.execute(call).await,
            ark_ports::ToolPreCallVerdict::Block { reason } => Err(ark_common::ArkError::new(
                ark_common::ArkErrorCode::Conflict,
                format!("plugin hook blocked tool `{}`: {reason}", call.tool),
            )),
        }
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
    /// Composition-root resolver for one steer entry's images and outcomes.
    /// `None` keeps steers text-only (child runs, runtimes without attachments).
    pub steer_attachment_resolver: Option<ark_agent::SteerAttachmentResolver>,
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
        steer_attachment_resolver: spec.steer_attachment_resolver,
    })
    .with_steering(spec.steering)
}

/// Who the child is, appended to the shared prefix before the skill tail.
pub(crate) const CHILD_ROLE_LINE: &str =
    "You are a child run. Finish the delegated task and answer with the result. \
     You cannot delegate further, and you have no `todo_write` tool: state your \
     steps directly in the reply instead of writing a plan.";

/// Append the workspace's project instructions to a seed's dynamic tail.
///
/// Shared by the parent and child paths so both sit in the same repository
/// voice (requirements/project-instructions.md). This is a dynamic-tail
/// message, never part of the static prefix: the text varies per workspace and
/// must not poison the byte-stable prompt-cache window.
pub(crate) fn push_project_instructions(
    workspace: &std::path::Path,
    messages: &mut Vec<ModelMessage>,
) {
    if let Some(text) = ark_workspace::project_instructions_seed(workspace) {
        messages.push(ModelMessage::system(text));
    }
}

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

    /// A one-line broker for order tests: appends a marker to the shared log
    /// file and returns the configured verdict.
    struct OrderBroker {
        log: PathBuf,
        verdict: ark_ports::ToolPreCallVerdict,
    }

    #[async_trait::async_trait]
    impl ark_ports::HookBroker for OrderBroker {
        async fn on_turn_pre_start(
            &self,
            _scope: ark_ports::HookScope<'_>,
            _source: &str,
        ) -> ark_common::Result<()> {
            Ok(())
        }

        async fn on_model_request(
            &self,
            _scope: ark_ports::HookScope<'_>,
            _request: &ark_ports::ModelRequest,
        ) -> ark_common::Result<ark_ports::ModelRequestTuning> {
            Ok(ark_ports::ModelRequestTuning::default())
        }

        async fn on_tool_pre_call(
            &self,
            _scope: ark_ports::HookScope<'_>,
            _tool: &str,
            _args_head: &str,
            _args_truncated: bool,
        ) -> ark_common::Result<ark_ports::ToolPreCallVerdict> {
            let mut log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log)
                .unwrap();
            use std::io::Write;
            log.write_all(b"plugin\n").unwrap();
            Ok(self.verdict.clone())
        }

        async fn observe_turn_end(
            &self,
            _scope: ark_ports::HookScope<'_>,
            _outcome: &ark_ports::TurnOutcomeFact,
        ) {
        }

        async fn observe_llm_call(
            &self,
            _scope: ark_ports::HookScope<'_>,
            _fact: &ark_ports::LlmCallFact<'_>,
        ) {
        }
    }

    /// Pinned order (requirements/plugin-hook-bus.md, 验收场景 6): a plugin
    /// decide runs BEFORE the shell `pre_tool_use` hook, which runs BEFORE the
    /// dispatch. Both sides append to one log file so the sequence is
    /// observable end to end; the empty registry makes the dispatch fail,
    /// which is enough to prove it ran last.
    #[tokio::test]
    async fn plugin_decide_runs_before_the_shell_hook_and_the_dispatch() {
        let log = std::env::temp_dir().join(format!(
            "ark-hook-order-{}-{}.log",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&log);
        let workspace = std::env::temp_dir().join("ark-hook-order-ws");
        let _ = std::fs::create_dir_all(&workspace);

        let config = Arc::new(ConfigStore::new(RuntimeConfig {
            hooks: ark_contract::HookSettings {
                pre_tool_use: vec![ark_contract::HookRule {
                    command: format!("echo shell>>{}", log.display()),
                    tools: Vec::new(),
                    timeout_ms: 0,
                }],
                ..ark_contract::HookSettings::default()
            },
            ..RuntimeConfig::default()
        }));
        let policy: Arc<dyn SandboxPolicy> =
            Arc::new(BuiltinSandboxPolicy::new(ark_common::Autonomy::Strict));
        let inner = HookedToolRunner::new(
            Arc::new(RegistryToolRunner::new(
                Arc::new(ToolRegistry::new()),
                Arc::clone(&policy),
                workspace.clone(),
            )),
            Arc::clone(&config),
            workspace.clone(),
            ThreadId::new(),
            TurnId::new(),
        );
        let runner = HookToolRunner::new(
            Arc::new(OrderBroker {
                log: log.clone(),
                verdict: ark_ports::ToolPreCallVerdict::Allow,
            }),
            ThreadId::new(),
            TurnId::new(),
            inner,
        );
        let call = ToolCallRequest::new("c1", "no_such_tool", serde_json::json!({"path": "x"}));
        // The empty registry cannot dispatch; the call's failure is expected
        // and irrelevant — the log order is what this test asserts.
        let _ = runner.execute(&call).await;
        let recorded = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            recorded.lines().collect::<Vec<_>>(),
            ["plugin", "shell"],
            "decide → shell hook → dispatch"
        );
        let _ = std::fs::remove_file(&log);
    }

    /// A blocking decide must stop the call before the shell hook and the
    /// dispatch ever run — otherwise the plugin veto is decorative.
    #[tokio::test]
    async fn a_blocking_decide_stops_the_call_before_the_shell_hook() {
        let log = std::env::temp_dir().join(format!(
            "ark-hook-order-{}-{}.log",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&log);
        let workspace = std::env::temp_dir().join("ark-hook-order-ws");
        let _ = std::fs::create_dir_all(&workspace);
        let config = Arc::new(ConfigStore::new(RuntimeConfig::default()));
        let policy: Arc<dyn SandboxPolicy> =
            Arc::new(BuiltinSandboxPolicy::new(ark_common::Autonomy::Strict));
        let inner = HookedToolRunner::new(
            Arc::new(RegistryToolRunner::new(
                Arc::new(ToolRegistry::new()),
                Arc::clone(&policy),
                workspace.clone(),
            )),
            Arc::clone(&config),
            workspace.clone(),
            ThreadId::new(),
            TurnId::new(),
        );
        let runner = HookToolRunner::new(
            Arc::new(OrderBroker {
                log: log.clone(),
                verdict: ark_ports::ToolPreCallVerdict::Block {
                    reason: "not today".into(),
                },
            }),
            ThreadId::new(),
            TurnId::new(),
            inner,
        );
        let call = ToolCallRequest::new("c1", "bash", serde_json::json!({"command": "ls"}));
        let err = runner
            .execute(&call)
            .await
            .expect_err("a blocked call is an error");
        assert!(err.message.contains("plugin hook blocked"));
        // The broker's own marker is there (it logged before deciding), but the
        // shell hook and the dispatch must never have appended anything.
        let recorded = std::fs::read_to_string(&log).unwrap_or_default();
        assert_eq!(recorded, "plugin\n", "nothing after the veto may run");
    }
}
