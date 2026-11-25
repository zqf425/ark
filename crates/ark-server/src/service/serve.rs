//! Listener and graceful shutdown (design §7.9).
//!
//! `bind` and `serve` are split so the composition root can print the real
//! bound address *before* the accept loop starts: a smoke test then never races
//! the listener, and the startup line it prints is the truth.

//!
//! The `crate::api` reference below is the service layer's only structural
//! dependency: binding a listener requires the router table. It carries no
//! business rule and is not a layering inversion like the ones this module
//! tree removed.
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
        let shutdown_state = state.clone();
        let app = router(state.clone());
        let result = axum::serve(self.listener, app)
            .with_graceful_shutdown(async move {
                // Either the explicit route or the OS signal ends the loop. Mark
                // shutdown here (not after axum finishes draining connections),
                // so a late child completion cannot create fresh parent work.
                tokio::select! {
                    _ = shutdown.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
                shutdown_state.begin_shutdown();
            })
            .await;
        drain(state).await?;
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

/// Cancel everything still running, then wait for it to reach a terminal state.
///
/// Cancelling **before** waiting is the whole point: a `Ctrl-C` that only stopped
/// accepting requests would leave turns waiting on a model response that nothing
/// will ever deliver, and the drain would always burn its full timeout before the
/// runtime dropped them mid-flight (design §7.1 / §7.9).
async fn drain(state: AppState) -> Result<()> {
    // 1. Reject late completion wakeups, then trip every cancel handle: parent
    // turns, pending continuations and child runs.
    state.begin_shutdown();
    state
        .turns
        .cancel_all_with_reason(crate::state::SHUTDOWN_CANCEL_REASON);
    let aborted_children = state.abort_children();
    if aborted_children > 0 {
        tracing::info!(aborted_children, "shutdown requested child aborts");
    }

    // 2. Wait for the terminal records the cancellations will produce.
    let deadline = tokio::time::Instant::now() + DRAIN_TIMEOUT;
    while (state.turns.active() > 0 || state.active_children() > 0)
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let turns_left = state.turns.active();
    let children_left = state.active_children();
    if turns_left > 0 || children_left > 0 {
        tracing::warn!(
            turns = turns_left,
            children = children_left,
            "shutdown drain timed out with work still running"
        );
        return Err(ArkError::new(
            ark_common::ArkErrorCode::RuntimeUnavailable,
            format!(
                "shutdown drain timed out with {turns_left} turns and {children_left} children unsettled"
            ),
        ));
    }
    tracing::info!("shutdown drain complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig};
    use std::sync::Arc;

    struct EmptyTools;

    impl ToolRegistryPort for EmptyTools {
        fn names(&self) -> Vec<String> {
            Vec::new()
        }

        fn schemas(&self) -> Vec<serde_json::Value> {
            Vec::new()
        }

        fn version(&self) -> u64 {
            0
        }
    }

    struct CancelRunner;

    #[async_trait::async_trait]
    impl crate::TurnRunner for CancelRunner {
        async fn run_turn(&self, job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
            job.cancel.cancelled().await;
            Ok(crate::TurnEnd::Completed)
        }
    }

    #[tokio::test]
    async fn shutdown_reports_unsettled_when_terminal_persistence_fails() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sessions =
            Arc::new(FileSessionStore::open(StoreConfig::new(dir.path())).expect("session store"));
        let gated = crate::api::handlers::test_support::GatedStore::new(
            sessions.clone() as Arc<dyn SessionStore>
        );
        gated.fail_append_at(2);
        let state = AppState::new(
            gated.clone(),
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            Arc::new(CancelRunner),
            dir.path().to_path_buf(),
            "test",
        );
        let thread = state.store.create_thread("t").await.expect("thread");
        let summary =
            crate::service::start_turn(&state, thread.thread_id, "hello".into(), Vec::new())
                .await
                .expect("turn starts");

        let error = drain(state.clone())
            .await
            .expect_err("shutdown must not report a durable close");

        assert_eq!(error.code, ark_common::ArkErrorCode::RuntimeUnavailable);
        assert!(state.turns.active() > 0);
        assert!(state
            .turns
            .get(summary.turn_id)
            .is_some_and(|slot| slot.ended_at.is_none()));
        assert_eq!(state.turns_completed(), 0);
    }
}
