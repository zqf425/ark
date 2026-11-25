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

use ark_ports::{SandboxDecision, Tool, ToolArgs, ToolConcurrency, ToolContext, ToolOutput};

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
    /// Incomplete encoded suffix withheld until the collector supplies its tail.
    decode_tail: Vec<u8>,
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

/// Decode only complete UTF-8 or GB18030, retaining an incomplete suffix for the next read.
/// Non-UTF-8 input still uses the shared legacy-codepage decoder.
fn decode_incremental_output(bytes: &[u8], final_chunk: bool) -> (String, Vec<u8>) {
    let complete = if final_chunk {
        bytes.len()
    } else {
        bytes.len() - ark_common::encoding::incomplete_encoded_suffix(bytes)
    };
    (
        decode_shell_output(&bytes[..complete]),
        bytes[complete..].to_vec(),
    )
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
    /// unrecognized form leaves the cwd unchanged, never wrong. The advanced
    /// path must exist and be enterable **before** it is committed: once a
    /// session cwd points at a non-directory string, every later call of the
    /// session fails before its command runs (the observed os error 123
    /// cascade), so an unusable target falls back to the current cwd.
    pub fn resolve_cwd(&self, workspace: &Path, command: &str) -> PathBuf {
        // The base is the *effective* cwd: a remembered path that has stopped
        // being enterable must not poison this call the way it poisoned the
        // whole session before (spawn refused, message named no fallback).
        let current = self.effective_cwd(workspace);
        let next = advance_cwd(workspace, &current, command);
        if next == current {
            return current;
        }
        let usable = std::fs::metadata(&next)
            .map(|meta| meta.is_dir())
            .unwrap_or(false);
        if !usable {
            return current;
        }
        if let Ok(mut state) = self.inner.lock() {
            // Committing over a stale remembered value is deliberate: the
            // fallback above already proved that value unusable in that case.
            state.cwd = Some(next.clone());
        }
        next
    }

    /// The cwd a call actually runs in: the remembered session cwd, falling
    /// back to the workspace when that path is no longer enterable.
    ///
    /// The remembered cwd is process-memory state and can go stale (a
    /// worktree removed between calls); a stale value used to surface as a
    /// spawn failure naming neither the path nor the fallback, which the
    /// model could not recover from.
    fn effective_cwd(&self, workspace: &Path) -> PathBuf {
        let remembered = self.cwd(workspace);
        let usable = remembered != workspace
            && std::fs::metadata(&remembered)
                .map(|meta| meta.is_dir())
                .unwrap_or(false);
        if usable {
            return remembered;
        }
        // Repair the session state instead of leaving a known-bad value in
        // place: the next call then starts from the workspace without paying
        // for the failed lookup again.
        if let Ok(mut state) = self.inner.lock() {
            if state.cwd.as_deref() == Some(remembered.as_path()) {
                state.cwd = None;
            }
        }
        workspace.to_path_buf()
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
        if retained_start > output.read_offset {
            // The withheld suffix itself fell out of the ring before this poll.
            output.decode_tail.clear();
        }
        let relative = (new_from - retained_start) as usize;
        let mut pending = std::mem::take(&mut output.decode_tail);
        pending.extend_from_slice(&snapshot_bytes[relative..]);
        let (chunk, decode_tail) = decode_incremental_output(&pending, output.finished);
        output.decode_tail = decode_tail;
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

/// The `cd`/`pushd` target of a command's first non-blank line, with one
/// layer of shell quotes removed.
///
/// Quoted targets are the observed real-world form (`cd "$USERPROFILE/x"`,
/// `cd "/d/code/repo"`): `split_whitespace` alone would hand the raw quotes
/// to the path joiner, and the recorded session cwd then contains literal
/// quote characters. Every following call of the session then dies on
/// Windows os error 123 before the command even runs -- three calls in one
/// day's sessions failed exactly there. This is deliberately *not* a shell
/// tokenizer: only one leading quote pair is stripped, no quote concatenation
/// (`cd "a"b` yields `a`, not `ab`) and no escape handling. Those inputs are
/// rare, and a divergence is safe -- the caller verifies the path before
/// committing, so at worst the session cwd advances to a prefix the shell
/// itself did not enter, and the model can re-`cd` explicitly.
fn cd_target(command: &str) -> Option<&str> {
    let first_line = command.lines().find(|line| !line.trim().is_empty())?;
    let trimmed = first_line.trim_start();
    let verb = trimmed.split_whitespace().next()?;
    if verb != "cd" && verb != "pushd" {
        return None;
    }
    let after_verb = trimmed[verb.len()..].trim_start();
    let &quote = after_verb.as_bytes().first()?;
    if quote == b'"' || quote == b'\'' {
        let inner = &after_verb[1..];
        let end = inner.find(quote as char)?;
        return Some(&inner[..end]);
    }
    after_verb.split_whitespace().next()
}

/// Advance `current` by one command's directory changes.
///
/// Recognized forms (one per command, first non-blank line): `cd -` is a
/// no-op here (the previous directory is not tracked), `cd`/`pushd` with a
/// single relative/absolute argument, quoted or bare. Everything else,
/// including commands that merely *contain* a `cd` after `&&` in a
/// non-leading position, keeps the cwd unchanged; the model can always chain
/// `cd dir && cmd` explicitly in the next call. Escapes are resolved by the
/// caller's path containment, so this function only normalizes.
///
/// A target that still contains `$` after unquoting is not tracked: the
/// *shell* expands it when the command runs, but this parser cannot know the
/// value, and recording the raw spelling once poisoned a whole session (the
/// recorded path never exists, so every later call died on Windows os error
/// 123). Not tracking is the safe divergence: the command still runs with the
/// shell-expanded `cd`, only the session cwd lags, and the model can re-`cd`
/// explicitly. The same asymmetry covers single-quoted `$` (which bash would
/// keep literal) and Windows names that legally contain `$` -- an
/// under-approximation of what the shell would do, never an over-claim.
fn advance_cwd(workspace: &Path, current: &Path, command: &str) -> PathBuf {
    let Some(target) = cd_target(command) else {
        return current.to_path_buf();
    };
    if target == "-" || target.is_empty() {
        return current.to_path_buf();
    }
    if target.contains('$') {
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
    // Drop `.` segments before the caller compares and commits: `cd .` joins
    // to `current/.`, which compares unequal to `current` and would otherwise
    // accumulate one segment per call.
    let normalized = PathBuf::from(ark_common::path::normalize(&joined.to_string_lossy()));
    if normalized == current {
        return current.to_path_buf();
    }
    normalized
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
    /// A session command owns a process; the shared manager keeps each task's
    /// buffer separate, so two commands do not corrupt each other's output.
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::Isolated
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
    /// Drains a ring buffer behind the manager's own lock.
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::ReadOnly
    }

    async fn execute(&self, _ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        let parsed: crate::domain::args::BashOutputArgs = args::parse("bash_output", &raw)?;
        Ok(self.snapshot(&parsed.task_id))
    }
}

impl BashOutputTool {
    fn snapshot(&self, task_id: &ShellTaskId) -> ToolOutput {
        let Some(snapshot) = self.session.task_output(task_id) else {
            return ToolOutput::err(unknown_task_message("bash_output", task_id));
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
    /// Signalling one task is idempotent and does not touch other tasks.
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::Isolated
    }

    async fn execute(&self, _ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        let parsed: crate::domain::args::BashKillArgs = args::parse("bash_kill", &raw)?;
        Ok(self.kill(&parsed.task_id))
    }
}

impl BashKillTool {
    fn kill(&self, task_id: &ShellTaskId) -> ToolOutput {
        if !self.session.kill_task(task_id) {
            return ToolOutput::err(unknown_task_message("bash_kill", task_id));
        }
        ToolOutput::ok(format!("task {task_id} terminated"))
    }
}

/// Structured error for an id this manager never registered.
///
/// The message is a contract, not prose: the only id that may appear in
/// `task_id` is one returned by `bash_session(run_in_background=true)` in this
/// process. `delegate_task` hands back a *child run* id, which belongs to
/// another namespace -- a model that mixes them up reads the old wording as
/// "the child failed" and redoes the work in the main thread (observed), so the
/// rejection states the two namespaces and the delivery path of each.
fn unknown_task_message(tool: &str, task_id: &ShellTaskId) -> String {
    format!(
        "unknown task id `{task_id}` for {tool}: this tool only accepts a background shell task \
         id returned by bash_session(run_in_background=true) in this process; the task never \
         started, was evicted, or the runtime restarted. A delegate_task child run id is a \
         different namespace and cannot be polled with bash_output or bash_kill: the child's \
         final answer arrives in a completion notice, so wait for it instead of re-running the \
         work"
    )
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
mod tests;
