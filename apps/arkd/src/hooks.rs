//! Agent lifecycle hooks (requirements/hooks.md) — the single execution host.
//!
//! Hooks are trusted configuration, not model input: the command comes from
//! `config.json` / a validated config patch, and the action description arrives
//! on stdin as JSON. Both the parent turn runner and the child runner call this
//! module's combinator, so the pre/post semantics exist exactly once.

use std::path::Path;
use std::time::Duration;

use ark_contract::{HookRule, HookSettings};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use ark_common::Result;
use ark_ports::ToolOutput;

/// Timeout applied when a rule does not set `timeout_ms`.
pub const DEFAULT_HOOK_TIMEOUT_MS: u64 = 10_000;
/// Hard ceiling for one hook invocation.
pub const MAX_HOOK_TIMEOUT_MS: u64 = 60_000;
/// Cap on hook text that reaches the model (block reason or post note).
const HOOK_TEXT_CAP_BYTES: usize = 4 * 1024;

/// Run the pre/post hooks around one tool dispatch.
///
/// `dispatch` is the inner execution; a blocked call never reaches it, but the
/// caller still records the refusal as the tool's result, so the reactor's
/// started/finished event pairing stays intact.
pub async fn run_around<F, Fut>(
    hooks: &HookSettings,
    workspace: &Path,
    payload: &Value,
    dispatch: F,
) -> Result<ToolOutput>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<ToolOutput>>,
{
    if let Some(reason) = run_pre_hooks(&hooks.pre_tool_use, payload, workspace).await {
        return Ok(ToolOutput {
            ok: false,
            content: reason,
            detail: Some("blocked_by_hook".to_string()),
        });
    }
    let mut output = dispatch().await?;
    if let Some(note) = run_post_hooks(&hooks.post_tool_use, payload, workspace).await {
        output.content.push_str("\n[post_tool_use hook]\n");
        output.content.push_str(&note);
    }
    Ok(output)
}

/// Execute `pre_tool_use` rules in order. `Some(reason)` blocks the call.
///
/// Exit code 2 is the only blocking outcome; anything else (other codes,
/// crashes, timeouts, spawn failures) logs a warning and lets the call proceed,
/// because a broken hook must not silently paralyse every tool call.
async fn run_pre_hooks(rules: &[HookRule], payload: &Value, workspace: &Path) -> Option<String> {
    let tool = payload.get("tool").and_then(Value::as_str)?;
    for rule in rules {
        if !rule_matches(rule, tool) {
            continue;
        }
        match invoke(rule, payload, workspace).await {
            Ok(output) if output.code == 2 => {
                let reason = if output.stderr.trim().is_empty() {
                    format!("tool `{tool}` was blocked by a pre_tool_use hook")
                } else {
                    cap_text(output.stderr.trim())
                };
                return Some(reason);
            }
            Ok(output) if output.code != 0 => {
                tracing::warn!(
                    tool,
                    code = output.code,
                    "pre_tool_use hook exited non-zero; continuing"
                );
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(tool, %error, "pre_tool_use hook failed to run; continuing");
            }
        }
    }
    None
}

/// Execute `post_tool_use` rules in order, collecting their stdout as a note.
async fn run_post_hooks(rules: &[HookRule], payload: &Value, workspace: &Path) -> Option<String> {
    let tool = payload.get("tool").and_then(Value::as_str)?;
    let mut notes = String::new();
    for rule in rules {
        if !rule_matches(rule, tool) {
            continue;
        }
        match invoke(rule, payload, workspace).await {
            Ok(output) if output.code == 0 && !output.stdout.trim().is_empty() => {
                if !notes.is_empty() {
                    notes.push('\n');
                }
                notes.push_str(&cap_text(output.stdout.trim()));
            }
            Ok(output) if output.code != 0 => {
                tracing::warn!(
                    tool,
                    code = output.code,
                    "post_tool_use hook exited non-zero; ignored"
                );
            }
            _ => {}
        }
    }
    (!notes.is_empty()).then_some(notes)
}

/// Whether a rule applies to this tool (empty filter = every tool).
fn rule_matches(rule: &HookRule, tool: &str) -> bool {
    rule.tools.is_empty() || rule.tools.iter().any(|name| name == tool)
}

struct HookProcessOutput {
    code: i32,
    stdout: String,
    stderr: String,
}

struct HookChild {
    #[cfg(windows)]
    process: std::process::Child,
    #[cfg(not(windows))]
    process: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
}

impl HookChild {
    #[cfg(not(windows))]
    fn from_tokio(mut child: tokio::process::Child) -> Self {
        Self {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
            process: child,
        }
    }

    #[cfg(windows)]
    fn from_std(mut child: std::process::Child) -> std::io::Result<Self> {
        Ok(Self {
            stdin: child
                .stdin
                .take()
                .map(tokio::process::ChildStdin::from_std)
                .transpose()?,
            stdout: child
                .stdout
                .take()
                .map(tokio::process::ChildStdout::from_std)
                .transpose()?,
            stderr: child
                .stderr
                .take()
                .map(tokio::process::ChildStderr::from_std)
                .transpose()?,
            process: child,
        })
    }

    fn start_kill(&mut self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            self.process.kill()
        }
        #[cfg(not(windows))]
        {
            self.process.start_kill()
        }
    }

    async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        #[cfg(windows)]
        {
            loop {
                if let Some(status) = self.process.try_wait()? {
                    return Ok(status);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        #[cfg(not(windows))]
        {
            self.process.wait().await
        }
    }
}

enum HookTree {
    #[cfg(not(windows))]
    Group(ark_common::process::ProcessTree),
    #[cfg(windows)]
    Job(windows::HookJob),
}

impl HookTree {
    fn disarm(&mut self) {
        match self {
            #[cfg(not(windows))]
            Self::Group(tree) => tree.disarm(),
            #[cfg(windows)]
            Self::Job(job) => job.disarm(),
        }
    }

    fn kill_tree(&self, child: &mut HookChild) {
        let killed = match self {
            #[cfg(not(windows))]
            Self::Group(tree) => tree.kill_group(),
            #[cfg(windows)]
            Self::Job(job) => job.terminate(),
        };
        if !killed {
            let _ = child.start_kill();
        }
    }
}

fn spawn_hook(command: std::process::Command) -> Result<(HookChild, HookTree)> {
    #[cfg(not(windows))]
    {
        let child = tokio::process::Command::from(command)
            .spawn()
            .map_err(|err| {
                ark_common::ArkError::internal(format!("hook could not start: {err}"))
            })?;
        let tree = HookTree::Group(ark_common::process::ProcessTree::adopt(child.id()));
        Ok((HookChild::from_tokio(child), tree))
    }
    #[cfg(windows)]
    {
        let mut command = command;
        let (child, job) = windows::spawn(&mut command).map_err(|err| {
            ark_common::ArkError::internal(format!("hook could not start: {err}"))
        })?;
        let child = HookChild::from_std(child).map_err(|err| {
            ark_common::ArkError::internal(format!("hook could not start: {err}"))
        })?;
        Ok((child, HookTree::Job(job)))
    }
}

/// Spawn one hook command with the payload on stdin.
async fn invoke(rule: &HookRule, payload: &Value, workspace: &Path) -> Result<HookProcessOutput> {
    let timeout = Duration::from_millis(if rule.timeout_ms == 0 {
        DEFAULT_HOOK_TIMEOUT_MS
    } else {
        rule.timeout_ms.min(MAX_HOOK_TIMEOUT_MS)
    });
    let mut command = shell(&rule.command);
    ark_common::process::isolate(&mut command);
    // All three pipes: payload in, decision text out. `wait_with_output` only
    // captures streams that are piped, so an inherit default would lose both.
    command
        .current_dir(workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, mut tree) = spawn_hook(command)?;
    if let Some(mut stdin) = child.stdin.take() {
        let body = serde_json::to_string(payload).unwrap_or_default();
        // A hook that never reads stdin must not deadlock us: the write races
        // the wait and a closed pipe is harmless.
        let _ = tokio::time::timeout(timeout / 2, stdin.write_all(body.as_bytes())).await;
    }
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let wait_for_output = async {
        let (status, stdout, stderr) = tokio::join!(
            child.wait(),
            read_optional(stdout.as_mut()),
            read_optional(stderr.as_mut())
        );
        Ok::<_, std::io::Error>(std::process::Output {
            status: status?,
            stdout: stdout?,
            stderr: stderr?,
        })
    };
    let output = match tokio::time::timeout(timeout, wait_for_output).await {
        Ok(Ok(output)) => {
            tree.disarm();
            output
        }
        Ok(Err(error)) => {
            return Err(ark_common::ArkError::internal(format!(
                "hook failed: {error}"
            )));
        }
        Err(_) => {
            tree.kill_tree(&mut child);
            let _ = child.wait().await;
            tree.disarm();
            return Err(ark_common::ArkError::internal(format!(
                "hook timed out after {timeout:?}"
            )));
        }
    };
    Ok(HookProcessOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

async fn read_optional<R>(reader: Option<&mut R>) -> std::io::Result<Vec<u8>>
where
    R: AsyncReadExt + Unpin,
{
    let mut bytes = Vec::new();
    if let Some(reader) = reader {
        reader.read_to_end(&mut bytes).await?;
    }
    Ok(bytes)
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
    };

    struct Handle(*mut std::ffi::c_void);

    // SAFETY: Windows kernel handles remain valid when moved or shared between
    // threads; `Drop` has exclusive ownership of each handle's final close.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                // SAFETY: every wrapped non-null handle is still owned here.
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    pub struct HookJob {
        handle: Handle,
        armed: bool,
    }

    impl HookJob {
        fn create() -> io::Result<Self> {
            // SAFETY: a null security descriptor and name request an unnamed job.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                handle: Handle(handle),
                armed: true,
            })
        }

        fn assign(&self, process: *mut std::ffi::c_void) -> io::Result<()> {
            // SAFETY: both handles were returned by Windows for live objects.
            if unsafe { AssignProcessToJobObject(self.handle.0, process) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn terminate(&self) -> bool {
            // SAFETY: the job handle remains valid for this guard's lifetime.
            unsafe { TerminateJobObject(self.handle.0, 1) != 0 }
        }

        pub fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for HookJob {
        fn drop(&mut self) {
            if self.armed {
                let _ = self.terminate();
            }
        }
    }

    pub fn spawn(command: &mut Command) -> io::Result<(Child, HookJob)> {
        // No descendant can be created before assignment: the direct process is
        // suspended, adopted by its private job, and only then resumed.
        command.creation_flags(CREATE_SUSPENDED);
        let mut child = command.spawn()?;
        let job = match HookJob::create() {
            Ok(job) => job,
            Err(error) => {
                abort(&mut child, None);
                return Err(error);
            }
        };
        if let Err(error) = job.assign(child.as_raw_handle()) {
            abort(&mut child, Some(&job));
            return Err(error);
        }
        if let Err(error) = resume(child.id()) {
            abort(&mut child, Some(&job));
            return Err(error);
        }
        Ok((child, job))
    }

    fn abort(child: &mut Child, job: Option<&HookJob>) {
        if let Some(job) = job {
            let _ = job.terminate();
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    fn resume(pid: u32) -> io::Result<()> {
        // SAFETY: TH32CS_SNAPTHREAD with process id zero snapshots all threads.
        let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) });
        if snapshot.0 == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = unsafe { zeroed() };
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        // SAFETY: the entry is initialized and the snapshot handle is valid.
        if unsafe { Thread32First(snapshot.0, &mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        loop {
            if entry.th32OwnerProcessID == pid {
                return resume_thread(entry.th32ThreadID);
            }
            // SAFETY: the same initialized entry and valid snapshot are reused.
            if unsafe { Thread32Next(snapshot.0, &mut entry) } == 0 {
                break;
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "suspended hook process has no primary thread",
        ))
    }

    fn resume_thread(thread_id: u32) -> io::Result<()> {
        // SAFETY: requesting only suspend/resume access does not mutate state.
        let thread = Handle(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) });
        if thread.0.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: Windows resumes this one thread and reports failure as u32::MAX.
        if unsafe { ResumeThread(thread.0) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Platform-appropriate shell for trusted commands (mirrors the bash tool).
#[cfg(windows)]
fn shell(command: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd
}

#[cfg(not(windows))]
fn shell(command: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c").arg(command);
    cmd
}

/// Cut hook text that reaches the model on a character boundary.
fn cap_text(text: &str) -> String {
    if text.len() <= HOOK_TEXT_CAP_BYTES {
        return text.to_string();
    }
    let mut end = HOOK_TEXT_CAP_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[hook output truncated]", &text[..end])
}

/// The stdin payload for one tool call (requirements/hooks.md §3).
pub fn hook_payload(
    phase: &str,
    tool: &str,
    arguments: &Value,
    thread_id: &ark_common::ThreadId,
    turn_id: &ark_common::TurnId,
) -> Value {
    json!({
        "phase": phase,
        "tool": tool,
        "arguments": arguments,
        "thread_id": thread_id.to_string(),
        "turn_id": turn_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::HookRule;

    fn rule(command: &str) -> HookRule {
        HookRule {
            command: command.into(),
            tools: Vec::new(),
            timeout_ms: 0,
        }
    }

    fn payload(tool: &str) -> Value {
        json!({
            "phase": "pre_tool_use",
            "tool": tool,
            "arguments": {"path": "a.txt"},
            "thread_id": "t",
            "turn_id": "u",
        })
    }

    fn settings(pre: Vec<HookRule>, post: Vec<HookRule>) -> HookSettings {
        HookSettings {
            pre_tool_use: pre,
            post_tool_use: post,
        }
    }

    async fn dispatch_ok() -> Result<ToolOutput> {
        Ok(ToolOutput {
            ok: true,
            content: "inner".into(),
            detail: None,
        })
    }

    #[tokio::test]
    async fn exit_zero_allows_the_call() {
        let hooks = settings(vec![rule("true")], vec![]);
        let out = run_around(&hooks, Path::new("."), &payload("read"), dispatch_ok)
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.content, "inner");
    }

    #[tokio::test]
    async fn exit_two_blocks_with_stderr_as_the_reason() {
        let command = if cfg!(windows) {
            "echo denied 1>&2 & exit /b 2"
        } else {
            "echo denied >&2; exit 2"
        };
        let hooks = settings(vec![rule(command)], vec![]);
        let out = run_around(&hooks, Path::new("."), &payload("read"), dispatch_ok)
            .await
            .unwrap();
        assert!(!out.ok);
        assert_eq!(out.content, "denied");
        assert_eq!(out.detail.as_deref(), Some("blocked_by_hook"));
    }

    #[tokio::test]
    async fn other_exit_codes_and_timeouts_still_allow() {
        let hooks = settings(vec![rule("exit 3"), rule("sleep 5"), rule("false")], vec![]);
        let out = run_around(&hooks, Path::new("."), &payload("read"), dispatch_ok)
            .await
            .unwrap();
        assert!(out.ok);
    }

    #[tokio::test]
    async fn a_timed_out_pre_hook_is_not_a_block() {
        let mut fast = rule("true");
        fast.timeout_ms = 100;
        let mut slow = rule("sleep 3");
        slow.timeout_ms = 100;
        let hooks = settings(vec![fast, slow], vec![]);
        let out = run_around(&hooks, Path::new("."), &payload("read"), dispatch_ok)
            .await
            .unwrap();
        assert!(out.ok);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_descendants_that_inherited_the_output_pipes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let pid_file = dir.path().join("grandchild.pid");
        let command = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let mut slow = rule(&command);
        slow.timeout_ms = 100;

        match invoke(&slow, &payload("read"), dir.path()).await {
            Ok(_) => panic!("the timed-out hook must fail"),
            Err(error) => assert!(error.message.contains("timed out")),
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let grandchild = loop {
            if let Ok(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok())
            {
                break pid;
            }
            assert!(std::time::Instant::now() < deadline, "child never started");
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        while std::process::Command::new("kill")
            .args(["-0", &grandchild.to_string()])
            .status()
            .is_ok_and(|status| status.success())
        {
            assert!(
                std::time::Instant::now() < deadline,
                "grandchild {grandchild} survived the hook timeout"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "spawned only by the Windows hook timeout regression"]
    fn windows_hook_descendant_fixture() {
        let Some(marker) = std::env::var_os("ARK_HOOK_DESCENDANT_MARKER") else {
            return;
        };
        let ready = std::env::var_os("ARK_HOOK_DESCENDANT_READY").expect("ready path");
        if std::env::var_os("ARK_HOOK_DESCENDANT_CHILD").is_some() {
            std::fs::write(&ready, b"ready").expect("signal descendant startup");
            std::thread::sleep(Duration::from_secs(3));
            std::fs::write(marker, b"survived").expect("write survivor marker");
            return;
        }

        let child = std::process::Command::new(std::env::current_exe().expect("test exe"))
            .args([
                "hooks::tests::windows_hook_descendant_fixture",
                "--exact",
                "--ignored",
            ])
            .env("ARK_HOOK_DESCENDANT_MARKER", &marker)
            .env("ARK_HOOK_DESCENDANT_READY", &ready)
            .env("ARK_HOOK_DESCENDANT_CHILD", "1")
            .spawn()
            .expect("spawn descendant fixture");
        let ready_path = std::path::PathBuf::from(&ready);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ready_path.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "descendant never started"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // Dropping (not waiting for) the child makes this foreground process
        // exit while its descendant retains the hook's stdout/stderr pipes.
        drop(child);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn timeout_kills_descendants_that_inherited_the_output_pipes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ready = dir.path().join("descendant.ready");
        let marker = dir.path().join("grandchild.marker");
        let executable = std::env::current_exe().expect("test executable");
        std::env::set_var("ARK_HOOK_DESCENDANT_READY", &ready);
        std::env::set_var("ARK_HOOK_DESCENDANT_MARKER", &marker);
        let command = format!(
            "call {} hooks::tests::windows_hook_descendant_fixture --exact --ignored --nocapture",
            executable.display()
        );
        let mut slow = rule(&command);
        slow.timeout_ms = 1_000;

        let result = invoke(&slow, &payload("read"), dir.path()).await;
        std::env::remove_var("ARK_HOOK_DESCENDANT_READY");
        std::env::remove_var("ARK_HOOK_DESCENDANT_MARKER");
        match result {
            Ok(output) => panic!(
                "the timed-out hook returned early: command={command:?} code={} stdout={:?} stderr={:?}",
                output.code, output.stdout, output.stderr
            ),
            Err(error) => assert!(error.message.contains("timed out")),
        }
        assert!(
            ready.exists(),
            "descendant must start before its parent exits"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            assert!(!marker.exists(), "a hook descendant survived the timeout");
            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn post_hook_stdout_is_appended_to_the_output() {
        let hooks = settings(vec![], vec![rule("echo formatted")]);
        let out = run_around(&hooks, Path::new("."), &payload("write"), dispatch_ok)
            .await
            .unwrap();
        assert!(out.content.contains("[post_tool_use hook]"));
        assert!(out.content.contains("formatted"));
    }

    #[tokio::test]
    async fn the_tool_filter_skips_unmatched_rules() {
        let mut scoped = rule("exit 2");
        scoped.tools = vec!["bash".into()];
        let hooks = settings(vec![scoped], vec![]);
        let out = run_around(&hooks, Path::new("."), &payload("read"), dispatch_ok)
            .await
            .unwrap();
        assert!(out.ok, "a rule scoped to bash must not gate `read`");
    }

    #[test]
    fn hook_text_is_capped_on_a_character_boundary() {
        let long = "汉".repeat(HOOK_TEXT_CAP_BYTES / 3 + 10);
        let capped = cap_text(&long);
        assert!(capped.len() < long.len());
        assert!(capped.ends_with("[hook output truncated]"));
    }
}
