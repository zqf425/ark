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
    sanitize_env(&mut command, ctx);

    let child = command.spawn().map_err(|err| {
        ark_common::ArkError::new(
            ark_common::ArkErrorCode::ToolExecutionFailed,
            format!("cannot start shell: {err}"),
        )
    })?;
    let output = match tokio::time::timeout(
        Duration::from_millis(timeout),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            return Ok(ToolOutput::err(format!("shell failed: {err}")));
        }
        Err(_) => {
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
        let dir = temp_workspace("bash-timeout");
        let arg = ToolArgs(serde_json::json!({
            "command": "ping -n 30 127.0.0.1",
            "timeout_ms": 200
        }));
        let started = std::time::Instant::now();
        let out = BashTool.execute(&ctx(&dir), arg).await.unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("timed out"), "got {}", out.content);
        assert!(started.elapsed() < Duration::from_secs(10));
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
