//! Persistent shell session and background tasks
//! (requirements/shell-sessions.md) — the single state host.
//!
//! The plain `bash` tool spawns one process per call: no cwd continuity and no
//! way to run a long task without parking the turn on a timeout. This module
//! adds process-lived continuity: the session remembers the working directory
//! across calls, and a background task detaches from the call so the model can
//! keep working and poll output later.
//!
//! Two deliberate limits keep the design honest (see the requirement's §2):
//! environment variables do **not** persist across calls (no portable shell
//! protocol on Windows), and only `cd`/`pushd`-style directory changes are
//! folded into session state. Session state is process memory, not durable
//! data — a restart resets it.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ark_ports::{SandboxDecision, Tool, ToolArgs, ToolContext, ToolOutput};

use super::bash::{default_shell_env, BashTool};
use crate::domain::args::{self, ShellTaskId};
use crate::exec::common::decode_shell_output;
use crate::policy::truncate::{cap_bytes, DEFAULT_LIMITS};

/// Capacity of one task's output ring. Beyond it, oldest bytes are dropped and
/// a marker is recorded, so a chatty server cannot eat unbounded memory.
pub const TASK_OUTPUT_CAP: usize = 64 * 1024;
/// Maximum retained tasks. Finished tasks are evicted oldest-first; running
/// tasks are never evicted, so polling a live task cannot fail by eviction.
pub const MAX_TASKS: usize = 16;

static NEXT_TASK_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_task_id() -> ShellTaskId {
    ShellTaskId::new(format!(
        "task_{}",
        NEXT_TASK_SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// One background task: a detached process tree plus its capped output buffer.
struct ShellTask {
    id: ShellTaskId,
    cwd: PathBuf,
    output: Arc<Mutex<TaskOutput>>,
    tree: ark_common::ProcessTree,
    child: Arc<Mutex<Option<std::process::Child>>>,
}

/// Capped, offset-tracked output shared between the collector thread and the
/// polling tool call. Crate-visible because `register_task`'s signature names
/// it; the type is never re-exported from the crate root.
#[derive(Default)]
pub(crate) struct TaskOutput {
    buf: VecDeque<u8>,
    dropped_prefix: bool,
    /// Total bytes removed from the front of `buf`.
    base_offset: u64,
    /// Absolute byte position returned by the previous poll.
    read_offset: u64,
    finished: bool,
    exit_code: Option<i32>,
}

impl TaskOutput {
    fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.buf.len() == TASK_OUTPUT_CAP {
                self.buf.pop_front();
                self.base_offset += 1;
                self.dropped_prefix = true;
            }
            self.buf.push_back(byte);
        }
    }

    fn text(&self) -> String {
        let bytes: Vec<u8> = self.buf.iter().copied().collect();
        decode_shell_output(&bytes)
    }
}

/// The per-runtime shell state: session cwd plus live background tasks.
#[derive(Default)]
pub struct ShellSessionManager {
    inner: Mutex<SessionState>,
}

#[derive(Default)]
struct SessionState {
    cwd: Option<PathBuf>,
    tasks: Vec<Arc<ShellTask>>,
}

impl ShellSessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// The session's current working directory, falling back to the workspace.
    pub fn cwd(&self, workspace: &Path) -> PathBuf {
        self.inner
            .lock()
            .ok()
            .and_then(|state| state.cwd.clone())
            .unwrap_or_else(|| workspace.to_path_buf())
    }

    /// Fold one command's directory changes into the session state.
    ///
    /// Returns the cwd the command should run in and the (possibly advanced)
    /// session cwd. Only line-level `cd`/`pushd` prefixes are recognized; the
    /// parse is intentionally crude because the fallback is safe: an
    /// unrecognized form leaves the cwd unchanged, never wrong.
    pub fn resolve_cwd(&self, workspace: &Path, command: &str) -> PathBuf {
        let current = self.cwd(workspace);
        let next = advance_cwd(workspace, &current, command);
        if next != current {
            if let Ok(mut state) = self.inner.lock() {
                state.cwd = Some(next.clone());
            }
        }
        next
    }

    /// Register a freshly spawned background task. The caller owns spawn; this
    /// owns bookkeeping (id, eviction, shared output buffer). The child handle
    /// arrives shared so `kill_task` can direct-kill even without group signal
    /// support, and the reaper thread takes it out on natural exit.
    pub(crate) fn register_task(
        &self,
        cwd: PathBuf,
        child: Arc<Mutex<Option<std::process::Child>>>,
        tree: ark_common::ProcessTree,
        output: Arc<Mutex<TaskOutput>>,
    ) -> ark_common::Result<ShellTaskId> {
        let mut state = match self.inner.lock() {
            Ok(state) => state,
            Err(_) => {
                terminate_unregistered(&tree, &child);
                return Err(ark_common::ArkError::internal(
                    "shell task registry lock poisoned",
                ));
            }
        };
        while state.tasks.len() >= MAX_TASKS {
            let Some(pos) = state
                .tasks
                .iter()
                .position(|task| task.output.lock().map(|o| o.finished).unwrap_or(false))
            else {
                drop(state);
                terminate_unregistered(&tree, &child);
                return Err(ark_common::ArkError::new(
                    ark_common::ArkErrorCode::ToolExecutionFailed,
                    format!("background task limit reached ({MAX_TASKS})"),
                ));
            };
            state.tasks.remove(pos);
        }
        let id = next_task_id();
        state.tasks.push(Arc::new(ShellTask {
            id: id.clone(),
            cwd,
            output,
            tree,
            child,
        }));
        Ok(id)
    }

    /// Incremental output for one task, plus its lifecycle status.
    ///
    /// `None` for an unknown (never started, evicted or process-restarted) id.
    pub fn task_output(&self, task_id: &ShellTaskId) -> Option<TaskSnapshot> {
        let state = self.inner.lock().ok()?;
        let task = state.tasks.iter().find(|task| &task.id == task_id)?;
        let mut output = task.output.lock().ok()?;
        let all = output.text();
        // Incremental read: bytes before the reader's offset are not resent.
        // The offset counts ring-buffer **logical** positions, so dropped
        // prefix bytes simply never reach a reader that was not there.
        let snapshot_bytes: Vec<u8> = output.buf.iter().copied().collect();
        let retained_start = output.base_offset;
        let retained_end = retained_start + snapshot_bytes.len() as u64;
        let new_from = output.read_offset.max(retained_start).min(retained_end);
        let relative = (new_from - retained_start) as usize;
        let chunk = decode_shell_output(&snapshot_bytes[relative..]);
        output.read_offset = retained_end;
        Some(TaskSnapshot {
            task_id: task_id.clone(),
            cwd: task.cwd.clone(),
            new_output: chunk,
            full_text: all,
            dropped_prefix: output.dropped_prefix,
            finished: output.finished,
            exit_code: output.exit_code,
        })
    }

    /// Kill one task's process group. Idempotent: a finished task reports
    /// `false` (nothing to kill) and stays queryable.
    pub fn kill_task(&self, task_id: &ShellTaskId) -> bool {
        let Ok(state) = self.inner.lock() else {
            return false;
        };
        let Some(task) = state.tasks.iter().find(|task| &task.id == task_id) else {
            return false;
        };
        let killed = task.tree.kill_group();
        if let Ok(mut guard) = task.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        if let Ok(mut output) = task.output.lock() {
            output.finished = true;
            output.exit_code.get_or_insert(-1);
        }
        killed
    }

    /// Whether the task is still registered (diagnostics/tests).
    #[cfg(test)]
    pub fn has_task(&self, task_id: &ShellTaskId) -> bool {
        self.inner
            .lock()
            .map(|state| state.tasks.iter().any(|task| &task.id == task_id))
            .unwrap_or(false)
    }
}

fn terminate_unregistered(
    tree: &ark_common::ProcessTree,
    child: &Arc<Mutex<Option<std::process::Child>>>,
) {
    tree.kill_group();
    if let Ok(mut guard) = child.lock() {
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_output_collector<R>(pipe: Option<R>, output: Arc<Mutex<TaskOutput>>)
where
    R: Read + Send + 'static,
{
    let Some(mut pipe) = pipe else {
        return;
    };
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if let Ok(mut guard) = output.lock() {
                        guard.push(&buf[..read]);
                    }
                }
            }
        }
    });
}

fn spawn_reaper(child: Arc<Mutex<Option<std::process::Child>>>, output: Arc<Mutex<TaskOutput>>) {
    std::thread::spawn(move || {
        let taken = child.lock().ok().and_then(|mut guard| guard.take());
        let code = taken.and_then(|mut child| child.wait().ok().and_then(|status| status.code()));
        if let Ok(mut guard) = output.lock() {
            guard.finished = true;
            guard.exit_code = code.or(Some(-1));
        }
    });
}

/// Immutable view handed to the `bash_output` tool call.
pub struct TaskSnapshot {
    pub task_id: ShellTaskId,
    pub cwd: PathBuf,
    /// Bytes appended since the previous poll of this task.
    pub new_output: String,
    /// The whole retained buffer (for a first read this equals `new_output`).
    pub full_text: String,
    pub dropped_prefix: bool,
    pub finished: bool,
    pub exit_code: Option<i32>,
}

/// Advance `current` by one command's directory changes.
///
/// Recognized forms (one per command, first match wins): `cd -` is a no-op
/// here (the previous directory is not tracked), `cd`, `pushd`, `popd` with a
/// single relative/absolute argument. Everything else — including commands
/// that merely *contain* a `cd` after `&&` in a non-leading position — keeps
/// the cwd unchanged; the model can always chain `cd dir && cmd` explicitly in
/// the next call. Escapes are resolved by the caller's path containment, so
/// this function only normalizes.
fn advance_cwd(workspace: &Path, current: &Path, command: &str) -> PathBuf {
    let first_line = command.lines().next().unwrap_or("").trim();
    let mut tokens = first_line.split_whitespace();
    let Some(verb) = tokens.next() else {
        return current.to_path_buf();
    };
    if verb != "cd" && verb != "pushd" {
        return current.to_path_buf();
    }
    let Some(target) = tokens.next() else {
        // Bare `cd` means home on Unix shells; without a tracked home the
        // safest interpretation is "stay".
        return current.to_path_buf();
    };
    if target == "-" {
        return current.to_path_buf();
    }
    let candidate = Path::new(target);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        current.join(candidate)
    };
    // Lexical containment: the real spawn resolves symlinks and re-checks the
    // boundary, so this is a fast-path filter for obvious escapes only.
    let Ok(lexical) = joined.strip_prefix(workspace) else {
        return current.to_path_buf();
    };
    if lexical.components().any(|c| c.as_os_str() == "..") {
        return current.to_path_buf();
    }
    joined
}

/// Sandbox baseline for the session tools: identical to `bash`.
pub const SESSION_BASELINE: SandboxDecision = SandboxDecision::NeedsApproval;

/// `bash_session` — run one command in the persistent session, or detach it.
pub struct BashSessionTool {
    session: Arc<ShellSessionManager>,
}

impl BashSessionTool {
    pub fn new(session: Arc<ShellSessionManager>) -> Self {
        Self { session }
    }
}

#[async_trait::async_trait]
impl Tool for BashSessionTool {
    fn name(&self) -> &'static str {
        "bash_session"
    }

    fn schema(&self) -> &'static serde_json::Value {
        crate::domain::schema::bash_session()
    }

    async fn execute(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        Ok(as_output(self.run(ctx, raw).await))
    }
}

impl BashSessionTool {
    async fn run(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        let parsed: crate::domain::args::BashSessionArgs = args::parse("bash_session", &raw)?;
        if parsed.command.trim().is_empty() {
            return Ok(ToolOutput::err("bash_session requires a non-empty command"));
        }
        if parsed.run_in_background {
            return self.spawn_background(ctx, &parsed);
        }
        // Foreground: the plain `bash` semantics (wait + timeout), but the cwd
        // comes from the session and a leading `cd` advances it.
        let cwd = self.session.resolve_cwd(&ctx.workspace, &parsed.command);
        let effective = ToolArgs(serde_json::json!({
            "command": parsed.command,
            "timeout_ms": parsed.timeout_ms,
            "cwd": cwd,
        }));
        BashTool.execute(ctx, effective).await
    }

    /// Detach one command: spawn, register, return the task id immediately.
    ///
    /// Uses the **std** Command on purpose: a detached task must not be bound
    /// to this async call's runtime lifetime, and the collector runs on plain
    /// threads. Group isolation and the env hygiene match the foreground tool.
    fn spawn_background(
        &self,
        ctx: &ToolContext,
        parsed: &crate::domain::args::BashSessionArgs,
    ) -> ark_common::Result<ToolOutput> {
        let cwd = self.session.resolve_cwd(&ctx.workspace, &parsed.command);
        super::bash::ensure_cwd(&cwd)?;
        let mut command = std_shell(&parsed.command)?;
        command
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // Same hygiene as the foreground tool: no inherited environment.
        command.env_clear();
        for (key, value) in default_shell_env() {
            command.env(key, value);
        }
        ark_common::isolate(&mut command);
        let mut child = command.spawn().map_err(|err| {
            ark_common::ArkError::new(
                ark_common::ArkErrorCode::ToolExecutionFailed,
                format!("cannot start background task: {err}"),
            )
        })?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let tree = ark_common::ProcessTree::adopt(Some(child.id()));
        // The registry holds a shared handle for a later `bash_kill`; the
        // reaper below owns the single `wait`. Kill after finish is a no-op
        // because the reaper has taken the child out.
        let shared_child = Arc::new(std::sync::Mutex::new(Some(child)));
        let output = Arc::new(std::sync::Mutex::new(TaskOutput::default()));
        // Plain threads collect both pipes until EOF; the tool call returns
        // long before either finishes.
        spawn_output_collector(stdout, Arc::clone(&output));
        spawn_output_collector(stderr, Arc::clone(&output));
        let id = self.session.register_task(
            cwd.clone(),
            Arc::clone(&shared_child),
            tree,
            Arc::clone(&output),
        )?;
        // Registration owns the process before the only waiter starts. If the
        // registry rejected it, register_task already killed and reaped it.
        spawn_reaper(shared_child, output);
        Ok(ToolOutput::ok(format!(
            "background task started: {id}\ncwd: {}\npoll output with bash_output; terminate with bash_kill",
            cwd.display()
        )))
    }
}

/// `bash_output` — incremental output plus status for one task.
pub struct BashOutputTool {
    session: Arc<ShellSessionManager>,
}

impl BashOutputTool {
    pub fn new(session: Arc<ShellSessionManager>) -> Self {
        Self { session }
    }
}

#[async_trait::async_trait]
impl Tool for BashOutputTool {
    fn name(&self) -> &'static str {
        "bash_output"
    }

    fn schema(&self) -> &'static serde_json::Value {
        crate::domain::schema::bash_output()
    }

    async fn execute(&self, _ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        let parsed: crate::domain::args::BashOutputArgs = args::parse("bash_output", &raw)?;
        Ok(self.snapshot(&parsed.task_id))
    }
}

impl BashOutputTool {
    fn snapshot(&self, task_id: &ShellTaskId) -> ToolOutput {
        let Some(snapshot) = self.session.task_output(task_id) else {
            return ToolOutput::err(format!(
                "unknown task id `{task_id}`: the task never started, was evicted, or the runtime restarted"
            ));
        };
        let mut body = String::new();
        if snapshot.dropped_prefix {
            body.push_str("[output truncated: older bytes were dropped]\n");
        }
        if snapshot.new_output.trim().is_empty() && !snapshot.finished {
            body.push_str("(no new output; task still running)\n");
        } else if !snapshot.new_output.trim().is_empty() {
            body.push_str(&cap_bytes(
                &snapshot.new_output,
                DEFAULT_LIMITS.max_bash_bytes,
            ));
            body.push('\n');
        } else {
            body.push_str("(no output)\n");
        }
        if snapshot.finished {
            body.push_str(&format!(
                "task finished, exit={}\n",
                snapshot.exit_code.unwrap_or(-1)
            ));
        } else {
            body.push_str("task running\n");
        }
        ToolOutput::ok(body)
    }
}

/// `bash_kill` — terminate one task's process group, idempotently.
pub struct BashKillTool {
    session: Arc<ShellSessionManager>,
}

impl BashKillTool {
    pub fn new(session: Arc<ShellSessionManager>) -> Self {
        Self { session }
    }
}

#[async_trait::async_trait]
impl Tool for BashKillTool {
    fn name(&self) -> &'static str {
        "bash_kill"
    }

    fn schema(&self) -> &'static serde_json::Value {
        crate::domain::schema::bash_kill()
    }

    async fn execute(&self, _ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        let parsed: crate::domain::args::BashKillArgs = args::parse("bash_kill", &raw)?;
        Ok(self.kill(&parsed.task_id))
    }
}

impl BashKillTool {
    fn kill(&self, task_id: &ShellTaskId) -> ToolOutput {
        if !self.session.kill_task(task_id) {
            return ToolOutput::err(format!(
                "unknown task id `{task_id}`: the task never started, was evicted, or the runtime restarted"
            ));
        }
        ToolOutput::ok(format!("task {task_id} terminated"))
    }
}

/// POSIX shell invocation for detached tasks. The foreground and background
/// variants intentionally use the same shell and argument convention.
fn std_shell(command: &str) -> ark_common::Result<std::process::Command> {
    #[cfg(windows)]
    {
        let path = super::bash::git_bash_path()?;
        let mut bash = std::process::Command::new(path);
        bash.arg("-c").arg(command);
        Ok(bash)
    }
    #[cfg(not(windows))]
    {
        let mut shell = std::process::Command::new("/bin/sh");
        shell.arg("-c").arg(command);
        Ok(shell)
    }
}

fn as_output(result: ark_common::Result<ToolOutput>) -> ToolOutput {
    match result {
        Ok(output) => output,
        Err(err) => ToolOutput::err(err.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ark-shell-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        dir
    }

    #[test]
    fn cwd_starts_at_the_workspace() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cwd-start");
        assert_eq!(manager.cwd(&ws), ws);
    }

    #[test]
    fn cd_advances_the_session_cwd() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-advance");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        let next = manager.resolve_cwd(&ws, "cd src");
        assert_eq!(next, ws.join("src"));
        assert_eq!(manager.cwd(&ws), ws.join("src"));
        // The next call starts from the advanced cwd.
        let again = manager.resolve_cwd(&ws, "echo hi");
        assert_eq!(again, ws.join("src"));
    }

    #[test]
    fn absolute_cd_outside_the_workspace_is_ignored() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-escape");
        let next = manager.resolve_cwd(&ws, "cd /etc");
        assert_eq!(next, ws, "an escape attempt must not move the session");
    }

    #[test]
    fn cd_dash_and_bare_cd_keep_the_cwd() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-dash");
        let next = manager.resolve_cwd(&ws, "cd -");
        assert_eq!(next, ws);
        let next = manager.resolve_cwd(&ws, "cd");
        assert_eq!(next, ws);
    }

    #[test]
    fn non_leading_cd_keeps_the_cwd() {
        let manager = ShellSessionManager::new();
        let ws = fixture("cd-nonleading");
        let next = manager.resolve_cwd(&ws, "echo hi && cd src");
        assert_eq!(next, ws);
    }

    #[test]
    fn pushd_advances_like_cd() {
        let manager = ShellSessionManager::new();
        let ws = fixture("pushd");
        std::fs::create_dir_all(ws.join("lib")).unwrap();
        let next = manager.resolve_cwd(&ws, "pushd lib");
        assert_eq!(next, ws.join("lib"));
    }

    #[test]
    fn task_ids_are_unique() {
        let manager = ShellSessionManager::new();
        let ws = fixture("task-ids");
        let output = Arc::new(Mutex::new(TaskOutput::default()));
        let id_a = manager
            .register_task(
                ws.clone(),
                shared_child(),
                orphan_tree(),
                Arc::clone(&output),
            )
            .unwrap();
        let id_b = manager
            .register_task(ws, shared_child(), orphan_tree(), output)
            .unwrap();
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn unknown_task_id_is_none() {
        let manager = ShellSessionManager::new();
        assert!(manager
            .task_output(&ShellTaskId::new("task_nope"))
            .is_none());
        assert!(!manager.kill_task(&ShellTaskId::new("task_nope")));
    }

    #[test]
    fn output_is_incremental_across_polls() {
        let manager = ShellSessionManager::new();
        let ws = fixture("incremental");
        let output = Arc::new(Mutex::new(TaskOutput::default()));
        let id = manager
            .register_task(ws, shared_child(), orphan_tree(), Arc::clone(&output))
            .unwrap();
        output.lock().unwrap().push(b"first\n");
        let first = manager.task_output(&id).unwrap();
        assert_eq!(first.new_output, "first\n");
        output.lock().unwrap().push(b"second\n");
        let second = manager.task_output(&id).unwrap();
        assert_eq!(second.new_output, "second\n");
        assert_eq!(second.full_text, "first\nsecond\n");
    }

    #[test]
    fn overflowed_output_drops_the_oldest_bytes() {
        let mut output = TaskOutput::default();
        let big = vec![b'x'; TASK_OUTPUT_CAP + 10];
        output.push(&big);
        assert!(output.dropped_prefix);
        assert_eq!(output.buf.len(), TASK_OUTPUT_CAP);
    }

    #[test]
    fn incremental_output_survives_ring_buffer_rotation() {
        let manager = ShellSessionManager::new();
        let ws = fixture("incremental-overflow");
        let output = Arc::new(Mutex::new(TaskOutput::default()));
        let id = manager
            .register_task(ws, shared_child(), orphan_tree(), Arc::clone(&output))
            .unwrap();
        output.lock().unwrap().push(&vec![b'x'; TASK_OUTPUT_CAP]);
        let first = manager.task_output(&id).unwrap();
        assert_eq!(first.new_output.len(), TASK_OUTPUT_CAP);
        output.lock().unwrap().push(b"new-tail");
        let second = manager.task_output(&id).unwrap();
        assert_eq!(second.new_output, "new-tail");
    }

    #[test]
    fn registering_a_seventeenth_running_task_is_rejected() {
        let manager = ShellSessionManager::new();
        let ws = fixture("task-capacity");
        let mut outputs = Vec::new();
        for _ in 0..MAX_TASKS {
            let output = Arc::new(Mutex::new(TaskOutput::default()));
            manager
                .register_task(
                    ws.clone(),
                    shared_child(),
                    orphan_tree(),
                    Arc::clone(&output),
                )
                .expect("task within capacity");
            outputs.push(output);
        }
        let overflow = manager.register_task(
            ws,
            shared_child(),
            orphan_tree(),
            Arc::new(Mutex::new(TaskOutput::default())),
        );
        assert!(overflow.is_err());
    }

    #[test]
    fn kill_is_idempotent_after_finish() {
        let manager = ShellSessionManager::new();
        let ws = fixture("kill-idempotent");
        let output = Arc::new(Mutex::new(TaskOutput::default()));
        let id = manager
            .register_task(ws, shared_child(), orphan_tree(), output)
            .unwrap();
        // The fake child is a real process that exits immediately; either kill
        // result is acceptable, but a second kill must not panic or hang.
        let _ = manager.kill_task(&id);
        let _ = manager.kill_task(&id);
        assert!(manager.has_task(&id));
    }

    /// A real child (instant exit) in the shared registry shape.
    fn shared_child() -> Arc<Mutex<Option<std::process::Child>>> {
        let child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                vec!["/C", "exit 0"]
            } else {
                vec![]
            })
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn fake child");
        Arc::new(Mutex::new(Some(child)))
    }

    /// An un-adopted tree handle: the fake child never ran isolated, so group
    /// signals report false and the direct-kill fallback covers teardown.
    fn orphan_tree() -> ark_common::ProcessTree {
        ark_common::ProcessTree::adopt(None)
    }
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    use ark_ports::{SandboxDecision, ToolContext};
    use std::collections::BTreeMap;
    use std::time::Duration;

    fn ctx(root: &Path) -> ToolContext {
        ToolContext {
            thread_id: ark_common::ThreadId::new(),
            turn_id: ark_common::TurnId::new(),
            workspace: root.to_path_buf(),
            sandbox: SandboxDecision::Unrestricted,
            env: BTreeMap::new(),
        }
    }

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ark-shelltool-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        dir
    }

    #[tokio::test]
    async fn foreground_command_runs_and_advances_cwd() {
        let dir = fixture("fg");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let session = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&session));
        // Advance via a leading cd, then read the session cwd back.
        let _ = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({"command": "cd src"})),
            )
            .await
            .unwrap();
        assert_eq!(session.cwd(&dir), dir.join("src"));
        // The second call starts from the advanced cwd; a bare command keeps it.
        let _ = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({"command": "echo hi"})),
            )
            .await
            .unwrap();
        assert_eq!(session.cwd(&dir), dir.join("src"));
    }

    #[tokio::test]
    async fn background_task_returns_an_id_and_collects_output() {
        let dir = fixture("bg");
        let session = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&session));
        let output_tool = BashOutputTool::new(Arc::clone(&session));
        let started = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({
                    "command": "echo bg-hello",
                    "run_in_background": true
                })),
            )
            .await
            .unwrap();
        assert!(started.ok, "{}", started.content);
        let task_id = started
            .content
            .lines()
            .find_map(|line| line.strip_prefix("background task started: "))
            .expect("task id line")
            .trim()
            .to_string();

        // Poll until the collector observes the echo; bounded wait.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut seen = String::new();
        loop {
            let snapshot = output_tool
                .execute(
                    &ctx(&dir),
                    ToolArgs(serde_json::json!({ "task_id": task_id })),
                )
                .await
                .unwrap();
            seen.push_str(&snapshot.content);
            if snapshot.content.contains("bg-hello") || snapshot.content.contains("finished") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "never saw output: {seen}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(seen.contains("bg-hello"), "{seen}");
        assert!(
            seen.contains("task finished") || seen.contains("task running"),
            "{seen}"
        );
    }

    #[tokio::test]
    async fn unknown_task_output_is_a_structured_error() {
        let dir = fixture("unknown");
        let tool = BashOutputTool::new(Arc::new(ShellSessionManager::new()));
        let out = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "task_id": "task_999" })),
            )
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("unknown task id"));
    }

    #[tokio::test]
    async fn kill_unknown_task_is_a_structured_error() {
        let dir = fixture("kill-unknown");
        let tool = BashKillTool::new(Arc::new(ShellSessionManager::new()));
        let out = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "task_id": "task_1" })),
            )
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn a_long_background_task_is_killable() {
        let dir = fixture("kill-running");
        let session = Arc::new(ShellSessionManager::new());
        let tool = BashSessionTool::new(Arc::clone(&session));
        let kill_tool = BashKillTool::new(Arc::clone(&session));
        let long = "sleep 30";
        let started = tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "command": long, "run_in_background": true })),
            )
            .await
            .unwrap();
        let task_id = started
            .content
            .lines()
            .find_map(|line| line.strip_prefix("background task started: "))
            .expect("task id line")
            .trim()
            .to_string();
        let killed = kill_tool
            .execute(
                &ctx(&dir),
                ToolArgs(serde_json::json!({ "task_id": task_id })),
            )
            .await
            .unwrap();
        assert!(killed.ok, "{}", killed.content);
        assert!(killed.content.contains("terminated"));
    }
}
