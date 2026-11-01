//! `arkd run`: one non-interactive turn over the real HTTP/SSE boundary
//! (requirements/headless-run.md).
//!
//! The driver reuses the exact `serve` assembly and then talks to itself over
//! loopback HTTP: create thread, start turn, stream the event log, shut down.
//! Going through the boundary instead of calling internals is deliberate —
//! authorization, gates and event semantics are then identical to the web
//! client by construction.

use std::time::Duration;

use serde_json::{json, Value};
use tokio::task::JoinHandle;

use crate::cli::RunArgs;

/// How long startup polling waits for `/health` before giving up.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);
/// SSE read idle cap: a turn whose stream stays silent this long is lost.
const STREAM_TIMEOUT: Duration = Duration::from_secs(600);

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
        .json(&json!({ "text": args.prompt }))
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
async fn stream_turn(
    client: &reqwest::Client,
    base: &str,
    thread_id: &str,
    since_seq: u64,
) -> Result<(), Failure> {
    let mut response = client
        .get(format!("{base}/v1/threads/{thread_id}/events"))
        .query(&[("since_seq", since_seq.to_string())])
        .timeout(STREAM_TIMEOUT)
        .send()
        .await
        .map_err(startup_of("event stream failed"))?;
    if !response.status().is_success() {
        return Err(Failure::Startup(format!(
            "event stream returned {}",
            response.status()
        )));
    }

    let mut buffer = String::new();
    loop {
        let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| Failure::Turn(format!("event stream broke: {err}")))?
        else {
            return Err(Failure::Turn(
                "event stream ended without a terminal event".into(),
            ));
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(index) = buffer.find("\n\n") {
            let frame: String = buffer.drain(..index + 2).collect();
            if let Some(outcome) = handle_frame(&frame)? {
                // Flush so a piped consumer sees every byte as it streams.
                use std::io::Write as _;
                let _ = std::io::stdout().flush();
                return outcome;
            }
        }
    }
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
    fn the_probe_returns_a_bindable_port() {
        let port = probe_port(0).unwrap();
        assert!(port > 0);
        // The explicit port wins unchanged.
        assert_eq!(probe_port(19_999).unwrap(), 19_999);
    }
}
