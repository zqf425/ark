//! Builtin `bash` tool (design §7.3).
//!
//! A shell command is the one builtin that can hang, so the timeout is not
//! optional: [`clamp_timeout`] always yields a positive bound, the child is
//! killed when it expires, and stdout/stderr are capped before they reach the
//! model context.

use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput};
use std::io;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::domain::{args, schema};
use crate::exec::common::{
    as_output, decode_shell_output, decode_shell_output_status, with_detail, workspace_path,
};
use crate::policy::truncate::{cap_bytes, clamp_timeout};
use crate::policy::DEFAULT_LIMITS;

/// Environment variables that are never forwarded to a child process.
const SECRET_MARKERS: [&str; 4] = ["KEY", "TOKEN", "SECRET", "PASSWORD"];

/// `bash`.
#[derive(Debug, Clone, Copy, Default)]
pub struct BashTool;

/// Enough lookahead to finish any UTF-8 or supported legacy-codepage character
/// that straddles the retained-prefix boundary.
const CAPTURE_LOOKAHEAD: usize = 4;

struct BoundedCapture {
    bytes: Vec<u8>,
    limit: usize,
    capacity: usize,
}

impl BoundedCapture {
    fn new(limit: usize) -> Self {
        let capacity = limit.saturating_add(CAPTURE_LOOKAHEAD);
        Self {
            bytes: Vec::with_capacity(capacity),
            limit,
            capacity,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        let retained = self
            .capacity
            .saturating_sub(self.bytes.len())
            .min(bytes.len());
        self.bytes.extend_from_slice(&bytes[..retained]);
    }

    fn exceeded(&self) -> bool {
        self.bytes.len() > self.limit
    }

    fn text(&self) -> String {
        if self.bytes.len() <= self.limit {
            return decode_shell_output(&self.bytes);
        }
        let sample = decode_sample(&self.bytes, self.limit);
        let earliest = self.limit.saturating_sub(CAPTURE_LOOKAHEAD - 1);
        for end in (earliest..=self.limit).rev() {
            let candidate = decode_shell_output_status(&self.bytes[..end]);
            if sample.text.starts_with(&candidate.text)
                && (candidate.encoding == sample.encoding
                    || candidate.encoding == ark_common::TextEncoding::Utf8)
            {
                return candidate.text;
            }
        }
        String::new()
    }
}

fn encoding_rank(encoding: ark_common::TextEncoding) -> u8 {
    match encoding {
        ark_common::TextEncoding::Utf8 => 0,
        ark_common::TextEncoding::Gb18030 => 1,
        ark_common::TextEncoding::Big5 => 2,
        ark_common::TextEncoding::ShiftJis => 3,
        ark_common::TextEncoding::EucKr => 4,
        ark_common::TextEncoding::Latin1 => 5,
    }
}

fn decode_sample(bytes: &[u8], limit: usize) -> ark_common::DecodedText {
    let mut best = decode_shell_output_status(&bytes[..limit]);
    let mut best_rank = encoding_rank(best.encoding);
    for end in (limit + 1..=bytes.len()).rev() {
        let candidate = decode_shell_output_status(&bytes[..end]);
        let rank = encoding_rank(candidate.encoding);
        if rank < best_rank {
            best_rank = rank;
            best = candidate;
        }
    }
    best
}

struct CapturedOutput {
    stdout: BoundedCapture,
    stderr: BoundedCapture,
    status: std::process::ExitStatus,
}

async fn capture_pipe<R>(pipe: Option<R>, limit: usize) -> io::Result<BoundedCapture>
where
    R: AsyncRead + Unpin,
{
    let mut capture = BoundedCapture::new(limit);
    let Some(mut pipe) = pipe else {
        return Ok(capture);
    };
    let mut buffer = [0u8; 8192];
    loop {
        let read = pipe.read(&mut buffer).await?;
        if read == 0 {
            return Ok(capture);
        }
        // Keep draining after the prefix is full so a chatty child never blocks
        // on a full stdout/stderr pipe while we are waiting for it to exit.
        capture.push(&buffer[..read]);
    }
}

async fn collect_output(
    child: &mut tokio::process::Child,
    limit: usize,
) -> io::Result<CapturedOutput> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (status, stdout, stderr) = tokio::try_join!(
        child.wait(),
        capture_pipe(stdout, limit),
        capture_pipe(stderr, limit)
    )?;
    Ok(CapturedOutput {
        stdout,
        stderr,
        status,
    })
}

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
    ensure_cwd(&cwd)?;
    let timeout = clamp_timeout(parsed.timeout_ms, &DEFAULT_LIMITS);
    let mut command = shell(&parsed.command)?;
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

    let mut child = command.spawn().map_err(|err| {
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
        collect_output(&mut child, DEFAULT_LIMITS.max_bash_bytes),
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

/// Refuse a working directory that cannot be entered, before spawning.
///
/// `current_dir` on a path that does not exist surfaces as a raw OS error from
/// `spawn` — on Windows 267 "the directory name is invalid", which names
/// neither the directory nor the reason. The model then has no way to tell a
/// stale worktree path from a missing permission bit; three calls in one day's
/// sessions died on that message alone. The check is not a substitute for the
/// spawn: it only turns the common "the path is gone" case into a message that
/// names the path.
pub(super) fn ensure_cwd(cwd: &std::path::Path) -> ark_common::Result<()> {
    match std::fs::metadata(cwd) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(ark_common::ArkError::validation(format!(
            "working directory `{}` is not a directory",
            cwd.display()
        ))),
        Err(err) => Err(ark_common::ArkError::validation(format!(
            "working directory `{}` is not accessible: {err}",
            cwd.display()
        ))),
    }
}

/// POSIX shell invocation for commands passed to the `bash` tool.
pub(super) fn shell(command: &str) -> ark_common::Result<tokio::process::Command> {
    #[cfg(windows)]
    {
        let path = git_bash_path()?;
        let mut bash = tokio::process::Command::new(path);
        bash.arg("-c").arg(command);
        Ok(bash)
    }
    #[cfg(not(windows))]
    {
        let mut shell = tokio::process::Command::new("/bin/sh");
        shell.arg("-c").arg(command);
        Ok(shell)
    }
}

/// Locate Git for Windows' POSIX shell without accidentally selecting the WSL
/// launcher (`C:\Windows\System32\bash.exe`). Git's `cmd` directory is on
/// PATH whenever `git` is available to normal Windows command prompts.
#[cfg(windows)]
pub(super) fn git_bash_path() -> ark_common::Result<std::path::PathBuf> {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths).find_map(|entry| {
                if !entry.join("git.exe").is_file() {
                    return None;
                }
                let candidate = entry.parent()?.join("bin").join("bash.exe");
                candidate.is_file().then_some(candidate)
            })
        })
        .ok_or_else(|| {
            ark_common::ArkError::new(
                ark_common::ArkErrorCode::ToolExecutionFailed,
                "Git Bash was not found on PATH; install Git for Windows or add its cmd directory to PATH",
            )
        })
}

/// Minimal environment a shell needs to resolve its own executables.
///
/// Values come from the host process (never from the model), and are only
/// forwarded when present. On Windows `SystemRoot`/`USERPROFILE` keep Git Bash
/// and standard utilities usable.
pub(super) fn default_shell_env() -> Vec<(&'static str, String)> {
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
fn render(output: &CapturedOutput, timeout_ms: u64) -> ToolOutput {
    let stdout = output.stdout.text();
    let stderr = output.stderr.text();
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
    let source_truncated = output.stdout.exceeded() || output.stderr.exceeded();
    let capped = if source_truncated && body.len() <= DEFAULT_LIMITS.max_bash_bytes {
        format!(
            "{body}\n[truncated: output exceeded {} bytes]",
            DEFAULT_LIMITS.max_bash_bytes
        )
    } else {
        cap_bytes(&body, DEFAULT_LIMITS.max_bash_bytes)
    };
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

    #[test]
    fn output_capture_is_bounded_while_excess_bytes_are_discarded() {
        let limit = 64;
        let mut capture = BoundedCapture::new(limit);
        capture.push(&vec![b'x'; limit * 16]);
        assert!(capture.bytes.len() <= limit + CAPTURE_LOOKAHEAD);
        assert_eq!(capture.text(), "x".repeat(limit));
    }

    #[test]
    fn bounded_output_does_not_split_utf8_or_legacy_codepage_characters() {
        let mut utf8 = BoundedCapture::new(8);
        utf8.push("1234567中中文中文".as_bytes());
        assert_eq!(utf8.text(), "1234567");

        let mut gbk = BoundedCapture::new(3);
        gbk.push(b"abc\xc4\xe3\xba\xc3\xc4\xe3");
        assert_eq!(gbk.text(), "abc");
    }

    #[test]
    fn four_byte_utf8_character_at_capture_limit_is_not_mojibake() {
        let mut capture = BoundedCapture::new(3);
        // The prefix ends exactly before a four-byte character. Sampling all
        // four following bytes is required to recognize the stream as UTF-8.
        capture.push("中\u{1f600}tail".as_bytes());
        assert_eq!(capture.text(), "中");
    }

    #[test]
    fn four_byte_gb18030_character_at_capture_limit_is_not_mojibake() {
        let mut capture = BoundedCapture::new(4);
        // The prefix ends exactly before a four-byte GB18030 character.
        capture.push(b"\xd6\xd0\xce\xc4\x94\x39\xfc\x36tail");
        assert_eq!(capture.text(), "中文");
    }

    #[test]
    fn sample_keeps_a_better_prefix_encoding() {
        let mut bytes = "中".as_bytes().to_vec();
        bytes.extend_from_slice(&[0xff; 4]);
        let mut capture = BoundedCapture::new(bytes.len() - 4);
        capture.push(&bytes);
        assert_eq!(capture.text(), "中");
    }

    #[tokio::test]
    async fn large_output_is_capped_after_bounded_capture() {
        let dir = temp_workspace("bash-large-output");
        let out = BashTool
            .execute(&ctx(&dir), echo("head -c 200000 /dev/zero | tr '\\0' x"))
            .await
            .unwrap();
        assert!(out.ok, "{}", out.content);
        assert!(
            out.content.contains("[truncated:"),
            "got {} bytes",
            out.content.len()
        );
        assert!(out.content.len() < DEFAULT_LIMITS.max_bash_bytes + 128);
    }

    #[tokio::test]
    async fn non_zero_exit_is_not_ok() {
        let dir = temp_workspace("bash-fail");
        let out = BashTool.execute(&ctx(&dir), echo("exit 3")).await.unwrap();
        assert!(!out.ok);
        assert!(out.detail.unwrap().contains("exit=3"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn runs_posix_commands_with_git_bash() {
        let dir = temp_workspace("bash-posix");
        let out = BashTool
            .execute(&ctx(&dir), echo("printf posix-shell"))
            .await
            .unwrap();
        assert!(out.ok, "{}", out.content);
        assert_eq!(out.content, "posix-shell");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn preserves_utf8_output_from_git_bash() {
        let dir = temp_workspace("bash-utf8");
        let out = BashTool
            .execute(&ctx(&dir), echo("printf '\u{4e2d}\u{6587}'"))
            .await
            .unwrap();
        assert!(out.ok, "{}", out.content);
        assert_eq!(out.content, "\u{4e2d}\u{6587}");
    }

    #[tokio::test]
    async fn timeout_kills_the_child() {
        // The `bash` tool accepts POSIX commands on every supported platform.
        let long_running = "sleep 30";
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
        let out = BashTool.execute(&ctx(&dir), echo("env")).await.unwrap();
        std::env::remove_var("ARK_TEST_SECRET");
        assert!(
            !out.content.contains("super-secret-value"),
            "{}",
            out.content
        );
        assert!(out.content.contains("PATH="), "PATH must remain usable");
    }

    #[tokio::test]
    async fn empty_command_is_refused() {
        let dir = temp_workspace("bash-empty");
        let out = BashTool.execute(&ctx(&dir), echo("   ")).await.unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn a_missing_working_directory_is_named_in_the_error() {
        // The raw OS failure here is Windows 267 "the directory name is
        // invalid", which names neither the path nor the reason; three calls in
        // one day's sessions ended there.
        let dir = temp_workspace("bash-missing-cwd");
        let missing = dir.join("gone").join("deeper");
        let arg = ToolArgs(serde_json::json!({
            "command": "echo x",
            "cwd": missing.to_string_lossy()
        }));
        let mut context = ctx(&dir);
        context.sandbox = ark_ports::SandboxDecision::Unrestricted;
        let out = BashTool.execute(&context, arg).await.unwrap();
        assert!(!out.ok);
        assert!(
            out.content.contains("working directory") && out.content.contains("gone"),
            "the error must name the directory: {}",
            out.content
        );
    }

    #[test]
    fn a_legacy_codepage_child_stream_decodes_instead_of_degrading() {
        // `decode_shell_output` is the one decoder both shell tools use; assert
        // the policy it delegates to, so a refactor cannot quietly restore
        // `from_utf8_lossy`, which turned every GBK line into a replacement
        // character and made the resulting diagnostics unreadable.
        let gbk = [0xC4u8, 0xE3, 0xBA, 0xC3]; // CP936 bytes for "你好"
        let decoded = decode_shell_output(&gbk);
        assert_eq!(decoded, "你好");
        assert!(
            !decoded.contains('�'),
            "a legacy-codepage stream must decode, not degrade"
        );
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
