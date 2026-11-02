//! `arkd run`: one non-interactive turn over the real HTTP/SSE boundary
//! (requirements/headless-run.md).
//!
//! The driver reuses the exact `serve` assembly and then talks to itself over
//! loopback HTTP: create thread, start turn, stream the event log, shut down.
//! Going through the boundary instead of calling internals is deliberate —
//! authorization, gates and event semantics are then identical to the web
//! client by construction.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::task::JoinHandle;

use crate::cli::RunArgs;

/// How long startup polling waits for `/health` before giving up.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);
/// Read-idle cap applied to a *single* SSE chunk read (not the whole stream):
/// the runtime sends keep-alive comments every `sse_heartbeat_secs`, so only a
/// genuinely wedged stream stays silent this long. Hitting it resumes from the
/// last seen seq instead of failing the turn.
const STREAM_TIMEOUT: Duration = Duration::from_secs(600);
/// Consecutive resumes without a new event seq. Every resume that advances the
/// seq resets the counter, so a long review that keeps producing events can run
/// for hours; the cap only stops a wedged runtime from being polled forever.
const RESUME_LIMIT: u32 = 50;
/// Pause between resumes so an instantly-closing runtime is polled, not hot-looped.
const RESUME_PAUSE: Duration = Duration::from_millis(100);

/// Exit codes of `arkd run`.
const EXIT_COMPLETED: i32 = 0;
const EXIT_TURN_FAILED: i32 = 1;
const EXIT_STARTUP_FAILED: i32 = 2;

pub async fn run(args: RunArgs) -> std::process::ExitCode {
    match drive(args).await {
        Ok(()) => std::process::ExitCode::from(EXIT_COMPLETED as u8),
        Err(Failure::Turn(message)) => {
            eprintln!("arkd: turn failed: {message}");
            std::process::ExitCode::from(EXIT_TURN_FAILED as u8)
        }
        Err(Failure::Startup(message)) => {
            eprintln!("arkd: {message}");
            std::process::ExitCode::from(EXIT_STARTUP_FAILED as u8)
        }
    }
}

#[derive(Debug)]
enum Failure {
    Startup(String),
    Turn(String),
}

async fn drive(args: RunArgs) -> Result<(), Failure> {
    let port = probe_port(args.port)?;
    let serve_args = crate::cli::ServeArgs {
        port,
        data_dir: args.data_dir.clone(),
        auth_token: args.auth_token.clone(),
        workspace: args.workspace.clone(),
    };
    let wiring = crate::wiring::build(&serve_args)
        .await
        .map_err(|err| Failure::Startup(format!("cannot start runtime: {err}")))?;
    let handle = ark_server::bind(wiring.state.clone(), port)
        .await
        .map_err(|err| Failure::Startup(format!("cannot bind 127.0.0.1:{port}: {err}")))?;
    let addr = handle.addr();
    eprintln!(
        "arkd run: listening on {addr}, data {}, model {}, workspace {}",
        args.data_dir.display(),
        wiring.model_label,
        args.workspace.display()
    );
    let server: JoinHandle<Result<(), ark_common::ArkError>> =
        tokio::spawn(async move { handle.serve().await });

    let outcome = one_turn(&args, addr).await;

    // Shutdown always runs, even after a failed turn: the drain converges any
    // in-flight work before the process exits.
    let client = client(&args);
    let _ = client
        .post(format!("http://{addr}/v1/shutdown"))
        .send()
        .await;
    let _ = server.await;

    outcome
}

/// Probe a free loopback port, or return the explicitly requested one.
///
/// The probe releases the socket before the runtime rebinds it; the window is
/// small and documented in the requirement design.
fn probe_port(requested: u16) -> Result<u16, Failure> {
    if requested != 0 {
        return Ok(requested);
    }
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|err| Failure::Startup(format!("cannot probe a free port: {err}")))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|err| Failure::Startup(format!("cannot read the probed port: {err}")))
}

fn client(args: &RunArgs) -> reqwest::Client {
    let mut builder = reqwest::Client::builder();
    if let Some(token) = &args.auth_token {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        builder = builder.default_headers(headers);
    }
    builder.build().unwrap_or_default()
}

/// One full turn: thread, turn, SSE stream, terminal event.
async fn one_turn(args: &RunArgs, addr: std::net::SocketAddr) -> Result<(), Failure> {
    let base = format!("http://{addr}");
    let client = client(args);
    wait_for_health(&client, &base).await?;

    let created = client
        .post(format!("{base}/v1/threads"))
        .json(&json!({ "title": "headless" }))
        .send()
        .await
        .map_err(|err| Failure::Startup(format!("thread create failed: {err}")))?;
    if !created.status().is_success() {
        return Err(Failure::Startup(format!(
            "thread create returned {}",
            created.status()
        )));
    }
    let thread: Value = created.json().await.map_err(startup_of("thread decode"))?;
    let thread_id = thread["thread_id"]
        .as_str()
        .ok_or_else(|| Failure::Startup("thread response has no id".into()))?
        .to_string();

    // A review pins the thread to `strict` autonomy: with nobody answering
    // approval gates, writes and shell become unreachable, which is what makes
    // the review findings-only (requirements/review-preset.md §2). The prompt
    // is the review instruction carrying the target diff.
    let turn_text = match &args.review {
        Some(target) => {
            let diff = compute_diff(target, &args.workspace)?;
            let pinned = client
                .patch(format!("{base}/v1/threads/{thread_id}"))
                .json(&json!({ "runtime": { "autonomy": "strict" } }))
                .send()
                .await
                .map_err(startup_of("autonomy pin failed"))?;
            if !pinned.status().is_success() {
                return Err(Failure::Startup(format!(
                    "autonomy pin returned {}",
                    pinned.status()
                )));
            }
            review_prompt(&target.label(), &diff)
        }
        None => args.prompt.clone(),
    };

    let detail: Value = client
        .get(format!("{base}/v1/threads/{thread_id}"))
        .send()
        .await
        .map_err(startup_of("thread detail fetch"))?
        .json()
        .await
        .map_err(startup_of("thread detail decode"))?;
    let last_seq = detail["last_seq"].as_u64().unwrap_or(0);

    let started = client
        .post(format!("{base}/v1/threads/{thread_id}/turns"))
        .json(&json!({ "text": turn_text }))
        .send()
        .await
        .map_err(startup_of("turn start failed"))?;
    if started.status().as_u16() != 202 {
        return Err(Failure::Startup(format!(
            "turn start returned {}",
            started.status()
        )));
    }

    stream_turn(&client, &base, &thread_id, last_seq).await
}

fn startup_of(what: &'static str) -> impl Fn(reqwest::Error) -> Failure {
    move |err| Failure::Startup(format!("{what}: {err}"))
}

/// Cap on the diff handed to the model, matching the tool-output budget.
const REVIEW_DIFF_CAP_BYTES: usize = 64 * 1024;

/// Compute the diff a review turn looks at, in the workspace.
///
/// Plain `git` CLI, run to completion with captured output; a missing repo or
/// an unknown revision is a startup failure, not a turn.
fn compute_diff(target: &crate::cli::ReviewTarget, workspace: &Path) -> Result<String, Failure> {
    use crate::cli::ReviewTarget;
    let mut combined = String::new();
    match target {
        ReviewTarget::Uncommitted => {
            // `git diff HEAD` covers tracked changes only; untracked files are
            // exactly what a pre-commit review usually looks at, so each one is
            // appended as a new-file diff.
            let tracked = git_run(workspace, &git_args(["diff", "HEAD"]))?;
            combined.push_str(&tracked);
            let untracked = git_capture(
                workspace,
                git_args(["ls-files", "--others", "--exclude-standard", "-z"]),
            )?;
            for path in untracked.split('\0').filter(|path| !path.is_empty()) {
                // `--no-index` exits 1 when the files differ, which is the
                // normal outcome here, not a failure.
                let diff = git_run_accepting(
                    workspace,
                    &git_args(["diff", "--no-index", "--", "/dev/null", path]),
                    &[1],
                )?;
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&diff);
            }
        }
        ReviewTarget::Base(branch) => {
            // PR-style: everything on HEAD since it diverged from the base.
            let merge_base =
                git_capture(workspace, git_args(["merge-base", branch.as_str(), "HEAD"]))?;
            combined.push_str(&git_run(workspace, &git_args(["diff", &merge_base]))?);
        }
        ReviewTarget::Commit(rev) => {
            combined.push_str(&git_run(
                workspace,
                &git_args(["diff", &format!("{rev}~1"), rev]),
            )?);
        }
    }
    if combined.trim().is_empty() {
        return Err(Failure::Startup(format!(
            "no diff found for {}",
            target.label()
        )));
    }
    if combined.len() <= REVIEW_DIFF_CAP_BYTES {
        return Ok(combined);
    }
    let mut end = REVIEW_DIFF_CAP_BYTES;
    while end > 0 && !combined.is_char_boundary(end) {
        end -= 1;
    }
    combined.truncate(end);
    combined.push_str("\n[diff truncated]\n");
    Ok(combined)
}

fn git_args<const N: usize>(args: [&str; N]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

fn git_run(workspace: &Path, args: &[String]) -> Result<String, Failure> {
    git_run_accepting(workspace, args, &[])
}

/// Run one git command; `extra_ok` lists exit codes that count as success
/// (`git diff --no-index` exits 1 to mean "differences found").
fn git_run_accepting(
    workspace: &Path,
    args: &[String],
    extra_ok: &[i32],
) -> Result<String, Failure> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .map_err(|err| Failure::Startup(format!("git {} failed: {err}", args.join(" "))))?;
    let code = output.status.code().unwrap_or(-1);
    if !output.status.success() && !extra_ok.contains(&code) {
        return Err(Failure::Startup(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_capture(workspace: &Path, args: Vec<String>) -> Result<String, Failure> {
    git_run(workspace, &args).map(|out| out.trim().to_string())
}

/// The model-visible instruction for one review turn.
fn review_prompt(label: &str, diff: &str) -> String {
    format!(
        "You are performing a read-only code review of the {label}. \
Report findings only; do not modify any file. \
Classify each finding as P0 (correctness or security, must fix), \
P1 (likely bug or clear defect) or P2 (polish). \
Cite the file and line for every finding. \
If there are no findings, say so explicitly.\n\
Below is the diff under review:\n\
```diff\n{diff}```"
    )
}

async fn wait_for_health(client: &reqwest::Client, base: &str) -> Result<(), Failure> {
    let deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if let Ok(response) = client.get(format!("{base}/health")).send().await {
            if response.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(Failure::Startup("runtime did not become healthy".into()))
}

/// Consume the SSE stream, forwarding text deltas to stdout until a terminal
/// event arrives.
///
/// The server closes an idle subscription after `sse_idle_timeout_secs` with an
/// explicit `sse_idle_timeout` event — a long review turn thinking past that
/// window is normal, so the stream is resumed from the last seen `id` instead
/// of being reported as a failure. The replay protocol makes resumption lossless
/// and duplicate-free: the server replays `(since_seq, watermark]` in order and
/// only then attaches to live. Failures before the first successful connect are
/// startup failures (exit 2); anything after the turn is underway is a turn
/// failure (exit 1).
async fn stream_turn(
    client: &reqwest::Client,
    base: &str,
    thread_id: &str,
    since_seq: u64,
) -> Result<(), Failure> {
    let mut since_seq = since_seq;
    let mut last_progress = since_seq;
    let mut stalled = 0u32;
    let mut attempt: u32 = 0;
    loop {
        // The connect itself needs the same budget as a read: a runtime that
        // accepts the TCP connection but never returns response headers would
        // otherwise hang `.send()` forever, past every resume safeguard.
        let sent = tokio::time::timeout(
            STREAM_TIMEOUT,
            client
                .get(format!("{base}/v1/threads/{thread_id}/events"))
                .query(&[("since_seq", since_seq.to_string())])
                .send(),
        )
        .await;
        let mut response = match sent {
            Err(_elapsed) => {
                return Err(stream_failure(
                    attempt,
                    "event stream connect timed out".into(),
                ))
            }
            Ok(Ok(response)) if response.status().is_success() => response,
            Ok(Ok(response)) => {
                let status = response.status();
                return Err(stream_failure(
                    attempt,
                    format!("event stream returned {status}"),
                ));
            }
            Ok(Err(err)) => {
                return Err(stream_failure(
                    attempt,
                    format!("event stream failed: {err}"),
                ))
            }
        };

        // Byte buffer, not String: multi-byte UTF-8 (review findings are often
        // Chinese) must not be split mid-character by a chunk boundary.
        let mut buffer: Vec<u8> = Vec::new();
        loop {
            match tokio::time::timeout(STREAM_TIMEOUT, response.chunk()).await {
                Err(_elapsed) => break, // read idle: resume from the last seq
                Ok(Ok(Some(chunk))) => {
                    buffer.extend_from_slice(&chunk);
                    while let Some(index) = frame_end(&buffer) {
                        let frame: Vec<u8> = buffer.drain(..index + 2).collect();
                        let frame = String::from_utf8_lossy(&frame);
                        // Monotonic on purpose: a stale id (possible after a
                        // rewind) must never move the cursor backwards, or the
                        // replay would re-print deltas.
                        if let Some(parsed) = frame_seq(&frame) {
                            since_seq = since_seq.max(parsed);
                        }
                        if let Some(outcome) = handle_frame(&frame)? {
                            // Flush so a piped consumer sees every byte as it streams.
                            use std::io::Write as _;
                            let _ = std::io::stdout().flush();
                            return outcome;
                        }
                    }
                }
                // The server closed the stream (idle timeout is the normal
                // case): resume from the last seen seq.
                Ok(Ok(None)) => break,
                Ok(Err(err)) => return Err(Failure::Turn(format!("event stream broke: {err}"))),
            }
        }

        attempt += 1;
        if since_seq == last_progress {
            stalled += 1;
            if stalled >= RESUME_LIMIT {
                return Err(Failure::Turn(
                    "event stream kept ending without a terminal event".into(),
                ));
            }
            // Only a resume that made no progress pauses: an idle close right
            // after real progress should re-subscribe without extra delay.
            tokio::time::sleep(RESUME_PAUSE).await;
        } else {
            stalled = 0;
            last_progress = since_seq;
        }
    }
}

/// Errors before the first successful connect mean the runtime never came up;
/// identical errors after the turn is underway mean the turn was lost.
fn stream_failure(attempt: u32, message: String) -> Failure {
    if attempt == 0 {
        Failure::Startup(message)
    } else {
        Failure::Turn(message)
    }
}

/// Byte offset of the SSE frame delimiter `\n\n`, or `None` while the frame is
/// still incomplete.
fn frame_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(2).position(|window| window == b"\n\n")
}

/// The `id:` field of an SSE frame — the authoritative seq for resumption.
fn frame_seq(frame: &str) -> Option<u64> {
    frame
        .lines()
        .find_map(|line| line.strip_prefix("id: ")?.trim().parse().ok())
}

/// Handle one SSE frame; `Ok(Some(...))` ends the stream.
fn handle_frame(frame: &str) -> Result<Option<Result<(), Failure>>, Failure> {
    let mut event_kind = String::new();
    let mut data = String::new();
    for line in frame.lines() {
        if line.starts_with(':') {
            continue; // keep-alive comment
        }
        if let Some(kind) = line.strip_prefix("event: ") {
            event_kind = kind.trim().to_string();
        }
        if let Some(payload) = line.strip_prefix("data: ") {
            data.push_str(payload);
        }
    }
    if event_kind.is_empty() {
        return Ok(None);
    }
    let parsed: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
    match event_kind.as_str() {
        "assistant_text_delta" => {
            if let Some(delta) = parsed["delta"].as_str() {
                print!("{delta}");
                use std::io::Write as _;
                let _ = std::io::stdout().flush();
            }
            Ok(None)
        }
        "turn_completed" => Ok(Some(Ok(()))),
        "turn_failed" => {
            let message = parsed["message"].as_str().unwrap_or("unknown").to_string();
            Ok(Some(Err(Failure::Turn(message))))
        }
        "turn_aborted" => {
            let reason = parsed["reason"].as_str().unwrap_or("aborted").to_string();
            Ok(Some(Err(Failure::Turn(reason))))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ReviewTarget;

    #[test]
    fn sse_frames_are_parsed_for_deltas_and_terminals() {
        let mut stream = String::new();
        // keep-alive comment frames must be skipped.
        stream.push_str(": keep-alive\n\n");
        stream.push_str(
            "id: 7\nevent: assistant_text_delta\ndata: {\"kind\":\"assistant_text_delta\",\"seq\":7,\"delta\":\"he\"}\n\n",
        );
        stream.push_str(
            "id: 8\nevent: assistant_text_delta\ndata: {\"kind\":\"assistant_text_delta\",\"seq\":8,\"delta\":\"llo\"}\n\n",
        );
        stream.push_str(
            "id: 9\nevent: turn_completed\ndata: {\"kind\":\"turn_completed\",\"seq\":9,\"thread_id\":\"t\",\"turn_id\":\"u\",\"timestamp\":\"2026-01-01T00:00:00Z\"}\n\n",
        );

        let mut out = Vec::new();
        let mut terminal = None;
        for frame in stream.split("\n\n").filter(|frame| !frame.is_empty()) {
            let frame = format!("{frame}\n\n");
            if let Ok(Some(outcome)) = handle_frame(&frame) {
                terminal = Some(outcome);
                break;
            }
            out.push(frame);
        }
        let _ = out;
        assert!(terminal.is_some());
        assert!(terminal.unwrap().is_ok());
    }

    #[test]
    fn a_failed_terminal_carries_the_message() {
        let frame = "id: 3\nevent: turn_failed\ndata: {\"kind\":\"turn_failed\",\"seq\":3,\"message\":\"boom\"}\n\n";
        let outcome = handle_frame(frame).unwrap().unwrap();
        match outcome {
            Err(Failure::Turn(message)) => assert_eq!(message, "boom"),
            other => panic!("expected a turn failure, got {other:?}"),
        }
    }

    #[test]
    fn a_data_line_without_a_known_event_is_ignored() {
        let frame = "id: 1\nevent: thread_created\ndata: {\"kind\":\"thread_created\"}\n\n";
        assert!(handle_frame(frame).unwrap().is_none());
        assert!(handle_frame(": only a comment\n\n").unwrap().is_none());
        assert!(handle_frame("\n\n").unwrap().is_none());
    }

    #[test]
    fn an_idle_timeout_frame_does_not_end_the_stream() {
        // The server sends this right before closing an idle subscription; the
        // caller must resume from the last `id`, never treat it as a terminal.
        let frame = "id: 12\nevent: sse_idle_timeout\ndata: {\"kind\":\"sse_idle_timeout\",\"seq\":12,\"thread_id\":\"t\"}\n\n";
        assert!(handle_frame(frame).unwrap().is_none());
    }

    #[test]
    fn frame_seq_parses_the_id_field() {
        assert_eq!(frame_seq("id: 7\nevent: x\ndata: {}\n\n"), Some(7));
        assert_eq!(frame_seq("event: x\ndata: {}\n\n"), None, "no id line");
        assert_eq!(frame_seq("id: not-a-number\nevent: x\n\n"), None);
        assert_eq!(
            frame_seq("id: 9  \nevent: x\n\n"),
            Some(9),
            "trailing space"
        );
    }

    #[test]
    fn frame_end_survives_multibyte_characters_across_chunks() {
        // One Chinese character is three bytes; a chunk boundary inside it must
        // not make the delimiter search miss or mis-split the frame.
        let full = "id: 1\nevent: assistant_text_delta\ndata: {\"delta\":\"中文字符\"}\n\n";
        let bytes = full.as_bytes();
        let boundary = bytes.len() - 6; // middle of the last multi-byte char
        let mut buffer = bytes[..boundary].to_vec();
        assert_eq!(frame_end(&buffer), None, "delimiter not arrived yet");
        buffer.extend_from_slice(&bytes[boundary..]);
        let end = frame_end(&buffer).expect("complete frame");
        assert_eq!(&buffer[end + 2..], b"");
        let frame = String::from_utf8_lossy(&buffer[..end + 2]);
        assert!(frame.contains("中文字符"));
    }

    #[test]
    fn the_probe_returns_a_bindable_port() {
        let port = probe_port(0).unwrap();
        assert!(port > 0);
        // The explicit port wins unchanged.
        assert_eq!(probe_port(19_999).unwrap(), 19_999);
    }

    /// One temp git repo: one committed file plus one uncommitted edit.
    fn git_repo(tag: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir_in(std::env::temp_dir()).expect("temp dir");
        std::fs::create_dir_all(dir.path().join(tag)).unwrap();
        let workspace = dir.path().join(tag);
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&workspace)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example")
                .output()
                .expect("git runs");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        std::fs::write(workspace.join("a.txt"), "line one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "first"]);
        (dir, workspace)
    }

    #[test]
    fn uncommitted_diff_covers_the_working_tree_change() {
        let (_dir, workspace) = git_repo("uncommitted");
        std::fs::write(workspace.join("a.txt"), "line one\nline two\n").unwrap();
        let diff = compute_diff(&ReviewTarget::Uncommitted, &workspace).unwrap();
        assert!(diff.contains("+line two"), "got {diff}");
        // An empty diff is a structured startup failure, not a turn.
        assert!(compute_diff(
            &ReviewTarget::Uncommitted,
            workspace.parent().expect("parent")
        )
        .is_err());
    }

    #[test]
    fn uncommitted_diff_includes_untracked_files() {
        // A brand-new file is invisible to `git diff HEAD`; a pre-commit
        // review must still see it.
        let (_dir, workspace) = git_repo("untracked");
        std::fs::write(workspace.join("brand-new.py"), "print('hi')\n").unwrap();
        let diff = compute_diff(&ReviewTarget::Uncommitted, &workspace).unwrap();
        assert!(diff.contains("brand-new.py"), "got {diff}");
        assert!(diff.contains("+print('hi')"), "got {diff}");
    }

    #[test]
    fn base_diff_uses_the_merge_base_and_commit_diff_one_revision() {
        let (_dir, workspace) = git_repo("base");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&workspace)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example")
                .output()
                .expect("git runs")
        };
        std::fs::write(workspace.join("a.txt"), "changed on main\n").unwrap();
        assert!(git(&["commit", "-aqm", "second"]).status.success());

        let commit = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();
        let diff = compute_diff(&ReviewTarget::Commit(commit), &workspace).unwrap();
        assert!(diff.contains("-line one"));
        assert!(diff.contains("+changed on main"));

        // The first commit is the merge base of HEAD, so `base:<first>` sees
        // the second commit's change.
        let first = String::from_utf8(git(&["rev-parse", "HEAD~1"]).stdout)
            .unwrap()
            .trim()
            .to_string();
        let base_diff = compute_diff(&ReviewTarget::Base(first), &workspace).unwrap();
        assert!(base_diff.contains("+changed on main"));
    }

    #[test]
    fn the_review_prompt_carries_the_target_and_constraints() {
        let prompt = review_prompt("uncommitted changes", "+line two");
        assert!(prompt.contains("read-only code review"));
        assert!(prompt.contains("do not modify any file"));
        assert!(prompt.contains("P0"));
        assert!(prompt.contains("uncommitted changes"));
        assert!(prompt.contains("+line two"));
    }
}
