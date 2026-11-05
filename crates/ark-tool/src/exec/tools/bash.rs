//! Builtin `bash` tool (design §7.3).
//!
//! A shell command is the one builtin that can hang, so the timeout is not
//! optional: [`clamp_timeout`] always yields a positive bound, the child is
//! killed when it expires, and stdout/stderr are capped before they reach the
//! model context.

use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput};
use std::process::Stdio;
use std::time::Duration;

use crate::domain::{args, schema};
use crate::exec::common::{as_output, with_detail, workspace_path};
use crate::policy::truncate::{cap_bytes, clamp_timeout};
use crate::policy::DEFAULT_LIMITS;

/// Environment variables that are never forwarded to a child process.
const SECRET_MARKERS: [&str; 4] = ["KEY", "TOKEN", "SECRET", "PASSWORD"];

/// `bash`.
#[derive(Debug, Clone, Copy, Default)]
pub struct BashTool;

#[async_trait::async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn schema(&self) -> &'static serde_json::Value {
        schema::bash()
    }

    async fn execute(&self, ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
        Ok(as_output(run(ctx, raw).await))
    }
}

async fn run(ctx: &ToolContext, raw: ToolArgs) -> ark_common::Result<ToolOutput> {
    let parsed: args::BashArgs = args::parse("bash", &raw)?;
    if parsed.command.trim().is_empty() {
        return Ok(ToolOutput::err("bash requires a non-empty command"));
    }
    let cwd = workspace_path(ctx, parsed.cwd.as_deref().or(Some(".")))?;
    let timeout = clamp_timeout(parsed.timeout_ms, &DEFAULT_LIMITS);
    let mut command = shell(&parsed.command);
    command
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // A tokio Command inherits the whole arkd environment by default. That
    // would hand model-controlled shell commands API keys, proxy credentials
    // and database passwords. Start from an empty environment and re-add only
    // the explicit, sanitized context snapshot plus the few variables a shell
    // needs to find its own tools.
    command.env_clear();
    for (key, value) in default_shell_env() {
        command.env(key, value);
    }
    sanitize_env(&mut command, ctx);
    // A shell command is a tree, not a process: `sh -c 'server &'` leaves the
    // server running, and `kill_on_drop` would only signal the shell. Marking
    // the shell as a group leader is what makes a later teardown reach the rest.
    ark_common::process::isolate(command.as_std_mut());

    let child = command.spawn().map_err(|err| {
        ark_common::ArkError::new(
            ark_common::ArkErrorCode::ToolExecutionFailed,
            format!("cannot start shell: {err}"),
        )
    })?;
    // Adopt before awaiting: the child pid is the group id, and a reaped pid may
    // already be reused by the time the wait finishes. While armed, dropping
    // this guard (a cancelled turn drops the whole future) kills the group.
    let mut tree = ark_common::process::ProcessTree::adopt(child.id());
    let output = match tokio::time::timeout(
        Duration::from_millis(timeout),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(output)) => {
            // The child is reaped, so its pid and group id are up for reuse.
            tree.disarm();
            output
        }
        Ok(Err(err)) => {
            return Ok(ToolOutput::err(format!("shell failed: {err}")));
        }
        Err(_) => {
            // The expired future was dropped together with the child handle, so
            // `kill_on_drop` already signalled the shell itself; the group
            // signal is what reaches the commands it had started.
            tree.kill_group();
            return Ok(with_detail(
                ToolOutput::err(format!(
                    "command timed out after {timeout} ms and was killed"
                )),
                "timeout",
            ));
        }
    };
    Ok(render(&output, timeout))
}

/// Platform-appropriate shell invocation.
fn shell(command: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

/// Minimal environment a shell needs to resolve its own executables.
///
/// Values come from the host process (never from the model), and are only
/// forwarded when present. On Windows `SystemRoot`/`USERPROFILE` keep `cmd`
/// and the standard utilities usable.
fn default_shell_env() -> Vec<(&'static str, String)> {
    const KEYS: &[&str] = &[
        "PATH",
        "HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SystemRoot",
        "USERPROFILE",
        "COMSPEC",
        "LANG",
        "LC_ALL",
    ];
    KEYS.iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (*key, value)))
        .collect()
}

/// Copy the context snapshot into the child, dropping obvious secrets.
fn sanitize_env(command: &mut tokio::process::Command, ctx: &ToolContext) {
    for (key, value) in &ctx.env {
        let upper = key.to_uppercase();
        if SECRET_MARKERS.iter().any(|marker| upper.contains(marker)) {
            continue;
        }
        command.env(key, value);
    }
}

/// Build the model-visible output and the on-the-side detail.
fn render(output: &std::process::Output, timeout_ms: u64) -> ToolOutput {
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let mut body = String::new();
    if !stdout.is_empty() {
        body.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("[stderr]\n");
        body.push_str(&stderr);
    }
    if body.is_empty() {
        body.push_str("(no output)");
    }
    let capped = cap_bytes(&body, DEFAULT_LIMITS.max_bash_bytes);
    let code = output.status.code().unwrap_or(-1);
    let detail = format!("exit={code} timeout_ms={timeout_ms}");
    if output.status.success() {
        with_detail(ToolOutput::ok(capped), detail)
    } else {
        ToolOutput {
            ok: false,
            content: capped,
            detail: Some(detail),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::testsupport::{ctx, temp_workspace};

    fn echo(command: &str) -> ToolArgs {
        ToolArgs(serde_json::json!({ "command": command }))
    }

    #[tokio::test]
    async fn captures_stdout_and_exit_success() {
        let dir = temp_workspace("bash-echo");
        let out = BashTool
            .execute(&ctx(&dir), echo("echo hello"))
            .await
            .unwrap();
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("hello"));
        assert!(out.detail.unwrap().contains("exit=0"));
    }

    #[tokio::test]
    async fn non_zero_exit_is_not_ok() {
        let dir = temp_workspace("bash-fail");
        let out = BashTool.execute(&ctx(&dir), echo("exit 3")).await.unwrap();
        assert!(!out.ok);
        assert!(out.detail.unwrap().contains("exit=3"));
    }

    #[tokio::test]
    async fn timeout_kills_the_child() {
        // `ping -n` is Windows-only syntax; on Unix a usage-error exit would
        // bypass the timeout path entirely, so sleep plays the long-running child.
        let long_running = if cfg!(windows) {
            "ping -n 30 127.0.0.1"
        } else {
            "sleep 30"
        };
        let dir = temp_workspace("bash-timeout");
        let arg = ToolArgs(serde_json::json!({
            "command": long_running,
            "timeout_ms": 200
        }));
        let started = std::time::Instant::now();
        let out = BashTool.execute(&ctx(&dir), arg).await.unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("timed out"), "got {}", out.content);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn host_secrets_are_not_inherited() {
        // The old code never called `env_clear()`, so any model-authored
        // `env` command could read the whole arkd process environment.
        std::env::set_var("ARK_TEST_SECRET", "super-secret-value");
        let dir = temp_workspace("bash-env");
        let out = BashTool
            .execute(&ctx(&dir), echo(if cfg!(windows) { "set" } else { "env" }))
            .await
            .unwrap();
        std::env::remove_var("ARK_TEST_SECRET");
        assert!(!out.content.contains("super-secret-value"), "{}", out.content);
        assert!(out.content.contains("PATH="), "PATH must remain usable");
    }

    #[tokio::test]
    async fn empty_command_is_refused() {
        let dir = temp_workspace("bash-empty");
        let out = BashTool.execute(&ctx(&dir), echo("   ")).await.unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn cwd_escape_is_refused() {
        let dir = temp_workspace("bash-escape");
        let arg = ToolArgs(serde_json::json!({ "command": "echo x", "cwd": "../.." }));
        let out = BashTool.execute(&ctx(&dir), arg).await.unwrap();
        assert!(!out.ok);
    }
}

/// Process-tree teardown, Unix only: the assertion needs signal inspection and a
/// shell whose grandchildren can be named. On Windows the platform strategy
/// (`taskkill /T /F`) is documented in `ark_common::process` but cannot be
/// asserted here.
#[cfg(all(test, unix))]
mod tree_tests {
    use super::*;
    use crate::exec::testsupport::{ctx, temp_workspace};
    use std::path::{Path, PathBuf};
    use std::process::Command as StdCommand;
    use std::time::Instant;

    /// Workspace plus the pid file the shell's background job reports into.
    fn fixture(name: &str) -> (PathBuf, PathBuf) {
        let dir = temp_workspace(name);
        let pid_file = dir.join("grandchild.pid");
        (dir, pid_file)
    }

    /// `sh -c` script whose backgrounded `sleep` is the grandchild under test.
    fn forking_command(pid_file: &Path, timeout_ms: u64) -> ToolArgs {
        ToolArgs(serde_json::json!({
            "command": format!("sleep 30 & echo $! > {}; wait", pid_file.display()),
            "timeout_ms": timeout_ms
        }))
    }

    fn read_pid(path: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok())
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "grandchild never reported a pid");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn is_alive(pid: u32) -> bool {
        StdCommand::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn wait_until_dead(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while is_alive(pid) {
            assert!(
                Instant::now() < deadline,
                "grandchild {pid} outlived the teardown"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A timeout must stop the whole tree, not just the shell.
    #[tokio::test]
    async fn timeout_kills_the_grandchild() {
        let (dir, pid_file) = fixture("bash-tree-timeout");
        let arg = forking_command(&pid_file, 500);
        let out = BashTool.execute(&ctx(&dir), arg).await.unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("timed out"), "got {}", out.content);
        wait_until_dead(read_pid(&pid_file));
    }

    /// A cancelled call drops the future; the guard must still kill the tree.
    #[tokio::test]
    async fn cancelling_the_call_kills_the_grandchild() {
        let (dir, pid_file) = fixture("bash-tree-cancel");
        let arg = forking_command(&pid_file, 60_000);
        // Bound to a `let`: the future borrows the context for its whole life.
        let tool_ctx = ctx(&dir);
        let call = BashTool.execute(&tool_ctx, arg);
        let cancelled = tokio::time::timeout(Duration::from_millis(500), call).await;
        assert!(cancelled.is_err(), "the call must still be running");
        wait_until_dead(read_pid(&pid_file));
    }
}
