//! Listener and graceful shutdown (design §7.9).
//!
//! `bind` and `serve` are split so the composition root can print the real
//! bound address *before* the accept loop starts: a smoke test then never races
//! the listener, and the startup line it prints is the truth.

use std::net::SocketAddr;
use std::time::Duration;

use ark_common::{ArkError, Result};
use tokio::net::TcpListener;

use crate::api::routes::router;
use crate::state::AppState;

/// Upper bound on how long shutdown waits for in-flight turns to land on disk.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// A bound listener plus everything needed to serve it.
pub struct AppHandle {
    listener: TcpListener,
    state: AppState,
}

impl AppHandle {
    /// The address actually bound (matters when the caller asked for port 0).
    pub fn addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 0)))
    }

    /// Serve until `/v1/shutdown` or a signal arrives, then drain.
    pub async fn serve(self) -> Result<()> {
        let state = self.state.clone();
        let mut shutdown = state.shutdown_receiver();
        let app = router(state.clone());
        let result = axum::serve(self.listener, app)
            .with_graceful_shutdown(async move {
                // Either the explicit route or the OS signal ends the loop.
                tokio::select! {
                    _ = shutdown.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            })
            .await;
        drain(state).await;
        result.map_err(|err| ArkError::internal(format!("server error: {err}")))
    }
}

/// Bind `port`, returning the handle without accepting yet.
pub async fn bind(state: AppState, port: u16) -> Result<AppHandle> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|err| {
            ArkError::new(
                ark_common::ArkErrorCode::RuntimeUnavailable,
                format!("cannot bind 127.0.0.1:{port}: {err}"),
            )
        })?;
    Ok(AppHandle { listener, state })
}

/// Wait for in-flight turns to write their terminal event, bounded by timeout.
async fn drain(state: AppState) {
    let deadline = tokio::time::Instant::now() + DRAIN_TIMEOUT;
    while state.turns.active() > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let left = state.turns.active();
    if left > 0 {
        tracing::warn!(left, "shutdown drain timed out with turns still running");
    } else {
        tracing::info!("shutdown drain complete");
    }
}
