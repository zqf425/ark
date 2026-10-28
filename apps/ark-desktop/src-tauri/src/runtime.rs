//! Desktop-only `arkd` supervision.
//!
//! The shell starts one local runtime when no listener already owns the port.
//! It deliberately does not import an ark crate: process lifecycle is its only
//! responsibility and Agent/provider/plugin decisions remain in `arkd`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_shell::{process::CommandEvent, ShellExt};

const RUNTIME_PORT: u16 = 18_791;
const RUNTIME_ADDRESS: &str = "127.0.0.1:18791";

/// The process started by this shell, if any.
///
/// Existing listeners are deliberately not recorded, so shutdown never kills a
/// user-managed runtime. The mutex only protects ownership transfer; it is not
/// held while waiting for a process to exit.
#[derive(Default)]
pub struct ManagedRuntime(Mutex<Option<tauri_plugin_shell::process::CommandChild>>);

/// Start the local runtime unless a verified `arkd` listener already owns its port.
///
/// A listener with a different health response is rejected rather than reused
/// or killed. That prevents a desktop window from talking to an unrelated local
/// service and preserves the single-writer invariant for its data directory.
pub fn launch(app: &AppHandle) -> tauri::Result<()> {
    if let Some(is_arkd) = probe_existing_listener() {
        if is_arkd {
            log::info!("reusing arkd already listening on {RUNTIME_ADDRESS}");
            return Ok(());
        }
        return Err(tauri::Error::Anyhow(anyhow::anyhow!(
            "desktop runtime port {RUNTIME_ADDRESS} is occupied by a non-arkd service"
        )));
    }

    let command = runtime_command(app)?;
    let (mut events, child) = command
        .spawn()
        .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;
    app.state::<ManagedRuntime>().set(child);

    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stdout(line) => {
                    log::info!("arkd: {}", String::from_utf8_lossy(&line))
                }
                CommandEvent::Stderr(line) => {
                    log::warn!("arkd: {}", String::from_utf8_lossy(&line))
                }
                CommandEvent::Terminated(payload) => log::warn!("arkd exited: {payload:?}"),
                _ => {}
            }
        }
    });

    Ok(())
}

/// `None` means the port is currently free; `Some(false)` is an unknown service.
fn probe_existing_listener() -> Option<bool> {
    let address = RUNTIME_ADDRESS
        .parse()
        .expect("the compiled-in runtime address is valid");
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(100)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(250)))
        .ok()?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .ok()?;

    let mut response = [0_u8; 2_048];
    let count = stream.read(&mut response).ok()?;
    Some(is_arkd_health_response(
        std::str::from_utf8(&response[..count]).unwrap_or_default(),
    ))
}

fn is_arkd_health_response(response: &str) -> bool {
    response.starts_with("HTTP/1.1 200")
        && response.contains("\"status\":\"ok\"")
        && response.contains("\"version\":")
}

/// Stop only the process this shell started, including its process group.
pub fn shutdown(app: &AppHandle) {
    if let Some(child) = app.state::<ManagedRuntime>().take() {
        if let Err(error) = child.kill() {
            log::warn!("failed to stop the managed arkd process: {error}");
        }
    }
}

impl ManagedRuntime {
    fn set(&self, child: tauri_plugin_shell::process::CommandChild) {
        let mut guard = self
            .0
            .lock()
            .expect("managed runtime mutex is not poisoned");
        *guard = Some(child);
    }

    fn take(&self) -> Option<tauri_plugin_shell::process::CommandChild> {
        self.0
            .lock()
            .expect("managed runtime mutex is not poisoned")
            .take()
    }
}

fn runtime_args(data_dir: &Path, workspace: &Path) -> [String; 7] {
    [
        "serve".to_string(),
        "--port".to_string(),
        RUNTIME_PORT.to_string(),
        "--data-dir".to_string(),
        data_dir.display().to_string(),
        "--workspace".to_string(),
        workspace.display().to_string(),
    ]
}

#[cfg(debug_assertions)]
fn runtime_command(app: &AppHandle) -> tauri::Result<tauri_plugin_shell::process::Command> {
    let root = repository_root();
    let data_dir = desktop_data_dir(app)?;
    Ok(app
        .shell()
        .command("cargo")
        .set_process_group(true)
        .current_dir(&root)
        .args(["run", "--package", "arkd", "--"])
        .args(runtime_args(&data_dir, &root)))
}

#[cfg(not(debug_assertions))]
fn runtime_command(app: &AppHandle) -> tauri::Result<tauri_plugin_shell::process::Command> {
    let workspace = default_workspace(app)?;
    let data_dir = desktop_data_dir(app)?;
    app.shell()
        .sidecar("arkd")
        .map(|command| {
            command
                .set_process_group(true)
                .args(runtime_args(&data_dir, &workspace))
        })
        .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))
}

#[cfg(debug_assertions)]
fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("desktop manifest has a repository root")
        .to_path_buf()
}

#[cfg(not(debug_assertions))]
fn default_workspace(app: &AppHandle) -> tauri::Result<PathBuf> {
    app.path().document_dir()
}

fn desktop_data_dir(app: &AppHandle) -> tauri::Result<PathBuf> {
    app.path().app_data_dir().map(|path| path.join("runtime"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_arguments_isolate_the_desktop_port_and_data_directory() {
        assert_eq!(
            runtime_args(Path::new("D:/runtime"), Path::new("D:/work")),
            [
                "serve".to_string(),
                "--port".to_string(),
                "18791".to_string(),
                "--data-dir".to_string(),
                "D:/runtime".to_string(),
                "--workspace".to_string(),
                "D:/work".to_string(),
            ]
        );
    }

    #[test]
    fn runtime_health_probe_rejects_the_legacy_live_shape() {
        assert!(is_arkd_health_response(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"status\":\"ok\",\"version\":\"0.1.0\",\"uptime_ms\":1}"
        ));
        assert!(!is_arkd_health_response(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"ok\":true,\"status\":\"live\"}"
        ));
    }
}
