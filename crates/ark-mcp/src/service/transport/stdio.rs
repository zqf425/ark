//! stdio transport: one child process, newline-delimited JSON-RPC (design 7.6.1).
//!
//! This is the default for local MCP servers. Framing is one JSON object per
//! line on the child's stdin/stdout, which is what the MCP spec requires for
//! stdio. stderr is inherited so server logs stay visible to the operator
//! instead of silently filling a pipe buffer and deadlocking the child.
//!
//! Locks are `tokio::sync::Mutex` because a guard is held across an `.await` on
//! the pipe. A `std::sync::Mutex` guard is not `Send`, so it cannot be held
//! there without breaking the `async_trait` future.

use crate::domain::jsonrpc::{JsonRpcId, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::service::transport::{McpTransport, TransportKind};
use ark_common::{ArkError, ArkErrorCode, Result};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

/// A running MCP server child process.
pub struct StdioTransport {
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
    child: Mutex<Option<Child>>,
    next_id: Mutex<i64>,
    closed: Mutex<bool>,
}

impl std::fmt::Debug for StdioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioTransport").finish_non_exhaustive()
    }
}

impl StdioTransport {
    /// Spawn `program` with `args`. The child inherits stderr.
    pub async fn spawn(program: &str, args: &[String]) -> Result<Self> {
        if program.trim().is_empty() {
            return Err(ArkError::validation("mcp stdio program is empty"));
        }
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|err| {
            ArkError::new(
                ArkErrorCode::RuntimeUnavailable,
                format!("failed to spawn mcp server {program}: {err}"),
            )
            .with_detail("mcp_transport_spawn_failed")
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ArkError::internal("mcp child has no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ArkError::internal("mcp child has no stdout"))?;
        Ok(Self {
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            child: Mutex::new(Some(child)),
            next_id: Mutex::new(1),
            closed: Mutex::new(false),
        })
    }

    /// Allocate a request id; callers rarely need this.
    pub async fn next_id(&self) -> JsonRpcId {
        let mut guard = self.next_id.lock().await;
        let id = *guard;
        *guard += 1;
        JsonRpcId::Number(id)
    }

    /// Write one framed message followed by a newline.
    async fn write_line(&self, payload: &str) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(payload.as_bytes()).await.map_err(|err| {
            ArkError::new(
                ArkErrorCode::RuntimeUnavailable,
                format!("mcp stdio write failed: {err}"),
            )
        })?;
        stdin.write_all(b"\n").await.map_err(|err| {
            ArkError::new(
                ArkErrorCode::RuntimeUnavailable,
                format!("mcp stdio write failed: {err}"),
            )
        })?;
        stdin.flush().await.map_err(|err| {
            ArkError::new(
                ArkErrorCode::RuntimeUnavailable,
                format!("mcp stdio flush failed: {err}"),
            )
        })
    }

    /// Read until a response with the expected id arrives.
    ///
    /// Servers may interleave notifications and responses for other requests,
    /// so mismatched ids are skipped rather than treated as errors.
    async fn read_response_for(&self, id: &JsonRpcId) -> Result<JsonRpcResponse> {
        let mut stdout = self.stdout.lock().await;
        let mut line = String::new();
        loop {
            line.clear();
            let read = stdout.read_line(&mut line).await.map_err(|err| {
                ArkError::new(
                    ArkErrorCode::RuntimeUnavailable,
                    format!("mcp stdio read failed: {err}"),
                )
            })?;
            if read == 0 {
                return Err(ArkError::new(
                    ArkErrorCode::RuntimeUnavailable,
                    "mcp server closed stdout",
                )
                .with_detail("mcp_transport_closed"));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Notifications and unrelated responses are not our answer.
            let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                continue;
            };
            if value.get("id").is_none() {
                continue;
            }
            let Ok(response) = serde_json::from_value::<JsonRpcResponse>(value) else {
                continue;
            };
            if &response.id == id {
                return Ok(response);
            }
        }
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Stdio
    }

    async fn request(&self, req: JsonRpcRequest) -> Result<JsonRpcResponse> {
        if *self.closed.lock().await {
            return Err(
                ArkError::new(ArkErrorCode::RuntimeUnavailable, "mcp transport closed")
                    .with_detail("mcp_transport_closed"),
            );
        }
        let payload = serde_json::to_string(&req)?;
        self.write_line(&payload).await?;
        self.read_response_for(&req.id).await
    }

    async fn notify(&self, note: JsonRpcNotification) -> Result<()> {
        let payload = serde_json::to_string(&note)?;
        self.write_line(&payload).await
    }

    async fn close(&self) -> Result<()> {
        {
            let mut closed = self.closed.lock().await;
            if *closed {
                return Ok(());
            }
            *closed = true;
        }
        let child = self.child.lock().await.take();
        if let Some(mut child) = child {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        Ok(())
    }
}
