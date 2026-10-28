//! Hard-timeout `git` invocation (design 7.9).
//!
//! Why this exists: on Windows a git process can block indefinitely (index lock
//! held by another process, a wedged credential helper, a network drive that
//! stopped answering). `Command::output()` would then hang the HTTP request that
//! asked for the workspace status. So every call here is bounded:
//!
//! ```text
//! spawn -> poll try_wait() every 10 ms -> at 3 s: kill() + wait()
//! ```
//!
//! Two additional guards keep a read-only inspection from mutating anything or
//! waiting on a human:
//!
//! * `GIT_OPTIONAL_LOCKS=0` stops `git status` from taking the index lock;
//! * `GIT_TERMINAL_PROMPT=0` turns a credential prompt into an immediate error.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Wall-clock limit for one git invocation.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(3);

/// Polling granularity while waiting for the child to exit.
pub const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// `CREATE_NO_WINDOW`: keep a console window from flashing when the desktop app
/// shells out. Ignored on other platforms.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Successful git output, already decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
}

impl GitOutput {
    /// Trimmed stdout, the form every caller wants for a single-value answer.
    pub fn stdout_trimmed(&self) -> &str {
        self.stdout.trim()
    }
}

/// Run `git -C <cwd> <args>` with a hard timeout.
///
/// Returns `None` when git is missing, cannot be spawned, exceeds
/// [`GIT_TIMEOUT`], or exits non-zero. Callers translate `None` into a contract
/// `None` field: "git could not tell us" is never reported as `false`.
pub fn run_git(cwd: &Path, args: &[&str]) -> Option<GitOutput> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat")
        .env("GIT_EXTERNAL_DIFF", "")
        .env("GIT_DIFF_OPTS", "--no-ext-diff --no-textconv --no-color")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command.spawn().ok()?;
    let stdout_reader = spawn_reader(child.stdout.take());
    let stderr_reader = spawn_reader(child.stderr.take());

    let status = wait_with_timeout(&mut child, GIT_TIMEOUT);
    // Drain both pipes even on timeout: `kill` closes the child end, so the
    // reader threads finish instead of leaking into the rest of the session.
    let stdout = join_reader(stdout_reader);
    let stderr = join_reader(stderr_reader);

    let status = status?;
    if !status.success() {
        return None;
    }
    Some(GitOutput { stdout, stderr })
}

/// Read one piped stream to EOF on its own thread.
///
/// Reading concurrently with `try_wait` matters: `git status --porcelain` in a
/// large repository can fill the OS pipe buffer, and a child blocked on a full
/// pipe would look like a hang until the timeout fired.
fn spawn_reader<R: Read + Send + 'static>(
    pipe: Option<R>,
) -> Option<std::thread::JoinHandle<Vec<u8>>> {
    let mut pipe = pipe?;
    Some(std::thread::spawn(move || {
        let mut buffer = Vec::new();
        // A read error still yields whatever arrived; the caller treats partial
        // output as usable because every consumer either trims or counts lines.
        let _ = pipe.read_to_end(&mut buffer);
        buffer
    }))
}

fn join_reader(handle: Option<std::thread::JoinHandle<Vec<u8>>>) -> String {
    let bytes = match handle {
        Some(handle) => handle.join().unwrap_or_default(),
        None => Vec::new(),
    };
    // Git prints whatever bytes the filesystem holds. A filename that is not
    // valid UTF-8 (GBK on a Chinese Windows host, raw bytes from a Linux
    // checkout) must not turn a valid answer into a failure, so decode lossily:
    // we only ever use ASCII refs, SHA hex, and line counts here.
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Poll for exit until the deadline, then kill and reap.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => return None,
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_trimmed_strips_trailing_newline() {
        let output = GitOutput {
            stdout: "true\n".to_string(),
            stderr: String::new(),
        };
        assert_eq!(output.stdout_trimmed(), "true");
    }

    #[test]
    fn missing_cwd_fails_without_panicking() {
        let missing = std::env::temp_dir().join("ark-workspace-definitely-missing-cwd");
        assert!(run_git(&missing, &["rev-parse", "HEAD"]).is_none());
    }
}
