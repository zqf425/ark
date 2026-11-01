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
use ark_common::{ArkError, ArkErrorCode, ProcessTree, Result};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

/// A spawned server process together with the process group it leads.
///
/// The two are kept in one value so the group handle is taken, reaped and
/// disarmed by the same code path that owns the child. Dropping it (a transport
/// dropped without `close` -- discovery failed, session cancelled) therefore
/// tears the group down instead of leaving the server's own children running.
struct ManagedChild {
    child: Child,
    tree: ProcessTree,
}

/// A running MCP server child process.
pub struct StdioTransport {
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
    child: Mutex<Option<ManagedChild>>,
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
        // An MCP server is usually a launcher (`npx`, a language runtime) whose
        // real work runs in descendants; `kill_on_drop` alone would signal the
        // launcher and orphan them, keeping the stdout pipe open forever.
        ark_common::process::isolate(command.as_std_mut());
        let mut child = command.spawn().map_err(|err| {
            ArkError::new(
                ArkErrorCode::RuntimeUnavailable,
                format!("failed to spawn mcp server {program}: {err}"),
            )
            .with_detail("mcp_transport_spawn_failed")
        })?;
        // Adopt before taking the pipes: the handle must exist while the child
        // pid is still reserved, and the child id doubles as the group id.
        let tree = ProcessTree::adopt(child.id());
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
            child: Mutex::new(Some(ManagedChild { child, tree })),
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
        let managed = self.child.lock().await.take();
        if let Some(managed) = managed {
            let ManagedChild {
                mut child,
                mut tree,
            } = managed;
            // Signal the group first, then reap. Children that outlived the
            // server would otherwise keep the inherited stdout pipe open; the
            // direct kill is the fallback for a group that already exited.
            tree.kill_tree(|| {
                let _ = child.start_kill();
            });
            let _ = child.wait().await;
            // Reaped: the pid (and group id) may now be reused by another
            // process, so stop signalling on drop.
            tree.disarm();
        }
        Ok(())
    }
}

/// Process-tree teardown, Unix only. The assertion inspects signals, so on
/// Windows the `taskkill /T /F` strategy documented in `ark_common::process`
/// stays unverified here.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command as StdCommand;
    use std::time::{Duration, Instant};

    /// Scratch directory for the pid file the server's child reports into.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ark-mcp-stdio-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// A "server" that behaves like a real launcher: it starts a long-lived
    /// child of its own and then waits, so killing only the launcher would leave
    /// that child -- and the stdout pipe it inherited -- behind.
    async fn spawn_with_grandchild(pid_file: &Path) -> StdioTransport {
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let args = vec!["-c".to_string(), script];
        StdioTransport::spawn("/bin/sh", &args)
            .await
            .expect("spawn the fake server")
    }

    async fn read_pid(path: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok())
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "child never reported a pid");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn is_alive(pid: u32) -> bool {
        StdCommand::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    async fn wait_until_dead(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while is_alive(pid) {
            assert!(
                Instant::now() < deadline,
                "server child {pid} outlived the teardown"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn close_kills_the_server_tree() {
        let dir = scratch("close");
        let pid_file = dir.join("child.pid");
        let transport = spawn_with_grandchild(&pid_file).await;
        let child = read_pid(&pid_file).await;
        transport.close().await.expect("close the transport");
        wait_until_dead(child).await;
    }

    #[tokio::test]
    async fn dropping_the_transport_kills_the_server_tree() {
        // Discovery failures and cancelled sessions drop the transport without
        // ever calling `close`; that path must tear the group down too.
        let dir = scratch("drop");
        let pid_file = dir.join("child.pid");
        let transport = spawn_with_grandchild(&pid_file).await;
        let child = read_pid(&pid_file).await;
        drop(transport);
        wait_until_dead(child).await;
    }
}
