//! Desktop-only `arkd` supervision.
//!
//! The shell starts one local runtime when no listener already owns the port.
//! It deliberately does not import an ark crate: process lifecycle is its only
//! responsibility and Agent/provider/plugin decisions remain in `arkd`.
//!
//! The main window is created only after `arkd` answers `/health`: the page's
//! first act is `GET /v1/threads`, and a window that loads before the runtime
//! binds its listener turns the dev proxy's connection refusal into a bare
//! `HTTP 500` error in the thread list (retry-only recovery). Gating the window
//! on the same health probe the shell already trusts removes the race at its
//! source instead of papering over it in the renderer.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_shell::{process::CommandEvent, ShellExt};

const RUNTIME_PORT: u16 = 18_791;
const RUNTIME_ADDRESS: &str = "127.0.0.1:18791";

/// How often the readiness watcher re-probes `/health`.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Fallback cap for the readiness wait. Debug builds reach here through
/// `cargo run`, which may compile first, so the default stays generous; an
/// exited child aborts the wait early, so a broken runtime never stalls the
/// window for the full budget.
const DEFAULT_READY_TIMEOUT_SECS: u64 = 120;
const READY_TIMEOUT_ENV: &str = "ARK_DESKTOP_READY_TIMEOUT_SECS";

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
    match probe_existing_listener() {
        // A verified arkd already owns the port: the page can load right away.
        Some(true) => return create_main_window(app),
        Some(false) => {
            return Err(tauri::Error::Anyhow(anyhow::anyhow!(
                "desktop runtime port {RUNTIME_ADDRESS} is occupied by a non-arkd service"
            )))
        }
        // Port is free: spawn the runtime and gate the window on readiness.
        None => {}
    }

    let command = runtime_command(app)?;
    let (mut events, child) = command
        .spawn()
        .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?;
    app.state::<ManagedRuntime>().set(child);

    // The runtime's exit is observable outside the logging task: the readiness
    // watcher aborts as soon as the child dies instead of burning its whole
    // timeout against a process that will never bind.
    let exited = Arc::new(AtomicBool::new(false));

    tauri::async_runtime::spawn({
        let exited = exited.clone();
        async move {
            while let Some(event) = events.recv().await {
                match event {
                    CommandEvent::Stdout(line) => {
                        log::info!("arkd: {}", String::from_utf8_lossy(&line))
                    }
                    CommandEvent::Stderr(line) => {
                        log::warn!("arkd: {}", String::from_utf8_lossy(&line))
                    }
                    CommandEvent::Terminated(payload) => {
                        log::warn!("arkd exited: {payload:?}");
                        exited.store(true, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
            // The event stream closing also means the process is gone.
            exited.store(true, Ordering::Relaxed);
        }
    });

    tauri::async_runtime::spawn(readiness_watch(app.clone(), exited));
    Ok(())
}

/// Wait for the runtime to answer `/health`, then create the main window.
///
/// A timeout or an exited child still opens the window so the renderer can
/// show its own error state instead of the app appearing not to start.
async fn readiness_watch(app: AppHandle, exited: Arc<AtomicBool>) {
    let timeout = ready_timeout();
    let ready = wait_until_ready(probe_existing_listener, exited, timeout).await;
    if !ready {
        log::warn!(
            "arkd did not answer /health within {}s; opening the window anyway",
            timeout.as_secs()
        );
    }
    if let Err(error) = create_main_window(&app) {
        log::error!("failed to create the main window: {error}");
    }
}

/// Poll the health probe until it reports a verified arkd, the process exits,
/// or the timeout elapses. Returns whether the runtime became ready.
async fn wait_until_ready<F>(mut probe: F, exited: Arc<AtomicBool>, timeout: Duration) -> bool
where
    F: FnMut() -> Option<bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if probe() == Some(true) {
            return true;
        }
        if exited.load(Ordering::Relaxed) {
            return false;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    }
}

/// Readiness budget, overridable for faster dev loops.
fn ready_timeout() -> Duration {
    let secs = std::env::var(READY_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(DEFAULT_READY_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Create the app window with the settings that used to live in
/// `tauri.conf.json` (`windows[0]`); the label must stay `"main"` so the
/// `default` capability keeps applying.
fn create_main_window(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("ark")
        .inner_size(1440.0, 920.0)
        .min_inner_size(960.0, 640.0)
        .resizable(true)
        // The default native drag-drop handler swallows file drops before the
        // webview sees them (Windows WebView2 replaces the HTML5 handler), so
        // the composer's drag-drop attachments never fire. Disabling it hands
        // drops back to the page; no Tauri `DragDropEvent` consumer exists.
        .disable_drag_drop_handler()
        .build()?;
    let _ = window.set_focus();
    Ok(())
}

/// `None` means the port is currently free; `Some(false)` is an unknown service.
fn probe_existing_listener() -> Option<bool> {
    let address = match RUNTIME_ADDRESS.to_socket_addrs() {
        Ok(mut addrs) => addrs.next()?,
        Err(_) => return None,
    };
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
    let (data_dir, workspace) = desktop_paths();
    Ok(app
        .shell()
        .command("cargo")
        .set_process_group(true)
        .current_dir(&root)
        .args(["run", "--package", "arkd", "--"])
        .args(runtime_args(&data_dir, &workspace)))
}

#[cfg(not(debug_assertions))]
fn runtime_command(app: &AppHandle) -> tauri::Result<tauri_plugin_shell::process::Command> {
    let (data_dir, workspace) = desktop_paths();
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

fn desktop_paths() -> (PathBuf, PathBuf) {
    #[cfg(windows)]
    let candidates = ["ARK_HOME", "USERPROFILE", "HOME"];
    #[cfg(not(windows))]
    let candidates = ["ARK_HOME", "HOME", "USERPROFILE"];
    let home = candidates
        .into_iter()
        .find_map(|key| std::env::var_os(key).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    desktop_paths_from(&home)
}

fn desktop_paths_from(home: &Path) -> (PathBuf, PathBuf) {
    let root = home.join(".arkd");
    (root.join("data"), root.join("workspace"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

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
    fn debug_and_release_paths_share_the_arkd_home_layout() {
        assert_eq!(
            desktop_paths_from(Path::new("/home/user")),
            (
                PathBuf::from("/home/user/.arkd/data"),
                PathBuf::from("/home/user/.arkd/workspace")
            )
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

    #[test]
    fn readiness_wait_succeeds_as_soon_as_the_probe_reports_arkd() {
        let probes = Arc::new(AtomicUsizeProbe::default());
        let counter = probes.clone();
        let ready = tauri::async_runtime::block_on(wait_until_ready(
            move || {
                counter.bump();
                Some(true)
            },
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(5),
        ));
        assert!(ready);
        assert_eq!(probes.count(), 1);
    }

    #[test]
    fn readiness_wait_gives_up_immediately_when_the_runtime_process_exits() {
        let probes = Arc::new(AtomicUsizeProbe::default());
        let counter = probes.clone();
        let ready = tauri::async_runtime::block_on(wait_until_ready(
            move || {
                counter.bump();
                None
            },
            Arc::new(AtomicBool::new(true)),
            Duration::from_secs(600),
        ));
        assert!(!ready);
        // Must not have burned the (long) timeout against a dead process.
        assert_eq!(probes.count(), 1);
    }

    #[test]
    fn readiness_wait_keeps_polling_until_the_timeout_elapses() {
        let probes = Arc::new(AtomicUsizeProbe::default());
        let counter = probes.clone();
        let ready = tauri::async_runtime::block_on(wait_until_ready(
            move || {
                counter.bump();
                None
            },
            Arc::new(AtomicBool::new(false)),
            Duration::from_millis(600),
        ));
        assert!(!ready);
        assert!(
            probes.count() >= 2,
            "the probe must be retried, not fired once"
        );
    }

    /// Tiny shared counter so the injected probe can report how often it ran.
    #[derive(Default)]
    struct AtomicUsizeProbe(AtomicUsize);
    impl AtomicUsizeProbe {
        fn bump(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn count(&self) -> usize {
            self.0.load(Ordering::Relaxed)
        }
    }
    impl Clone for AtomicUsizeProbe {
        fn clone(&self) -> Self {
            Self(AtomicUsize::new(self.0.load(Ordering::Relaxed)))
        }
    }
}
