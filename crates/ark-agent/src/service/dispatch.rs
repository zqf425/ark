//! Concurrent tool execution for one batch (design §7.1 `DispatchingTools`).
//!
//! Batching decisions belong to `policy::parallel`; this module only runs what
//! it was given and measures each call. Kept separate so the reactor stays a
//! dispatcher and this stays a tiny, testable helper.
//!
//! Calls are driven by one spawned forwarder task that pushes each finished
//! execution into a channel as soon as it completes (requirements/
//! parallel-tools-v2-streaming-settlement.md §1). The channel has room for the
//! whole batch, so a `send` never blocks and the forwarder's `join_all` polling
//! is never stalled by the consumer.

use std::sync::Arc;
use std::time::Instant;

use crate::domain::tool::{ToolCallRequest, ToolExecution};
use crate::exec::tool_runner::ToolRunner;
use crate::service::reactor::execution_of;

/// Slot tag of the panic sentinel: no real call occupies `usize::MAX`.
pub const PANIC_SLOT: usize = usize::MAX;

/// One settled call, tagged with its slot in the caller's input order.
///
/// The slot rides the channel so the consumer can place history rows in the
/// model's submission order regardless of completion order (requirements §1);
/// the slot also identifies durable-open bookkeeping when provider call ids repeat.
pub struct SettledCall {
    pub slot: usize,
    pub call: ToolCallRequest,
    pub execution: ToolExecution,
}

/// The live settlement stream of one batch.
///
/// `recv` yields each call as soon as it finishes (completion order); the
/// stream ends when every call settled. `abort` kills the forwarder: every
/// in-flight tool future is dropped — a bash's `kill_on_drop` guard thereby
/// kills the child process, preserving the v1 cancel semantics (§2.2) — while
/// results already in the channel remain receivable, so the consumer can drain
/// what completed before the abort (§2.3).
pub struct SettlementStream {
    receiver: tokio::sync::mpsc::Receiver<SettledCall>,
    forwarder: tokio::task::JoinHandle<()>,
}

impl Drop for SettlementStream {
    fn drop(&mut self) {
        // The stream owns the forwarder. A journal append error can return
        // before the batch drains; detaching that task would leave tool
        // futures (and their child processes) alive with no consumer.
        self.forwarder.abort();
    }
}

impl SettlementStream {
    /// Await the next settled call; `None` after the batch fully drained.
    pub async fn next(&mut self) -> Option<SettledCall> {
        self.receiver.recv().await
    }

    /// Kill in-flight calls, keeping already-delivered results receivable.
    pub fn abort(&mut self) {
        self.forwarder.abort();
    }

    /// True once the forwarder task ended in a panic instead of draining
    /// normally; the consumer must fail every call it has not settled yet
    /// (§2.5). Only meaningful after the stream ended. A cancelled forwarder
    /// (the §2.2 abort) is not an error.
    pub async fn join(&mut self) -> Result<(), String> {
        let handle = &mut self.forwarder;
        match handle.await {
            Ok(()) => Ok(()),
            Err(err) if err.is_cancelled() => Ok(()),
            Err(err) => Err(err.to_string()),
        }
    }
}

/// Run every call in `calls` concurrently, delivering each execution as soon
/// as it finishes. Results arrive in completion order; each item carries the
/// call's slot in the caller's input order plus the request itself, so the
/// consumer can settle events immediately and still place history rows in the
/// model's submission order (requirements/parallel-tools-v2 §1).
///
/// A tool future panicking is caught by the task boundary: the stream still
/// ends, and [`SettlementStream::join`] reports the panic so the consumer can
/// synthesize failures for the calls that never settled (§2.5) instead of the
/// panic unwinding the caller.
pub fn run_calls_streaming(
    tools: Arc<dyn ToolRunner>,
    calls: Vec<ToolCallRequest>,
) -> SettlementStream {
    let total = calls.len();
    let (sender, receiver) = tokio::sync::mpsc::channel(total.max(1));
    let forwarder = tokio::spawn(async move {
        let futures = calls.into_iter().enumerate().map(|(slot, call)| {
            let tools = Arc::clone(&tools);
            let sender = sender.clone();
            async move {
                let start = Instant::now();
                let output = tools.execute(&call).await;
                let duration_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                let execution = execution_of(&call, output, duration_ms);
                // While the stream is alive, a failed send must not panic the
                // forwarder. Dropping the stream aborts the task via its Drop
                // guard, so an early journal error cannot detach live tools.
                let _ = sender
                    .send(SettledCall {
                        slot,
                        call,
                        execution,
                    })
                    .await;
            }
        });
        futures::future::join_all(futures).await;
    });
    SettlementStream {
        receiver,
        forwarder,
    }
}

/// Run every call in `calls` concurrently and return the executions in input
/// order. Thin wrapper over [`run_calls_streaming`]: the streaming form is the
/// single concurrency driver; this only reorders, so the two cannot drift.
pub async fn run_calls(
    tools: Arc<dyn ToolRunner>,
    calls: Vec<ToolCallRequest>,
) -> Vec<ToolExecution> {
    let total = calls.len();
    let mut stream = run_calls_streaming(tools, calls);
    let mut settled = Vec::with_capacity(total);
    while let Some(item) = stream.next().await {
        settled.push(item);
    }
    settled.retain(|item| item.slot != PANIC_SLOT);
    settled.sort_by_key(|item| item.slot);
    settled.into_iter().map(|item| item.execution).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ports::ToolOutput;

    fn call(id: &str, tool: &str) -> ToolCallRequest {
        ToolCallRequest::new(id, tool, serde_json::json!({ "path": "a" }))
    }

    #[tokio::test]
    async fn results_preserve_input_order() {
        struct Mixed;
        #[async_trait::async_trait]
        impl ToolRunner for Mixed {
            async fn execute(&self, call: &ToolCallRequest) -> ark_common::Result<ToolOutput> {
                // Reverse completion order: c2 settles before c1.
                if call.call_id == "c2" {
                    Ok(ToolOutput::ok("second-call-first"))
                } else {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    Ok(ToolOutput::ok("first-call-second"))
                }
            }
        }
        let runner = Arc::new(Mixed);
        let executions = run_calls(runner, vec![call("c1", "read"), call("c2", "read")]).await;
        assert_eq!(executions.len(), 2);
        assert_eq!(executions[0].call_id, "c1");
        assert_eq!(executions[0].content, "first-call-second");
        assert_eq!(executions[1].call_id, "c2");
    }

    #[tokio::test]
    async fn streaming_arrives_in_completion_order() {
        struct Mixed;
        #[async_trait::async_trait]
        impl ToolRunner for Mixed {
            async fn execute(&self, call: &ToolCallRequest) -> ark_common::Result<ToolOutput> {
                if call.call_id == "c2" {
                    Ok(ToolOutput::ok("fast"))
                } else {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    Ok(ToolOutput::ok("slow"))
                }
            }
        }
        let mut stream = run_calls_streaming(
            Arc::new(Mixed),
            vec![call("c1", "read"), call("c2", "read")],
        );
        let first = stream.next().await.expect("first settled call");
        assert_eq!(first.call.call_id, "c2", "the fast call settles first");
        assert_eq!(
            first.slot, 1,
            "its slot still records the submission position"
        );
        let second = stream.next().await.expect("second settled call");
        assert_eq!(second.call.call_id, "c1");
        assert!(
            stream.next().await.is_none(),
            "the stream ends after the batch"
        );
    }

    #[tokio::test]
    async fn abort_keeps_already_delivered_results_receivable() {
        struct SlowNever;
        #[async_trait::async_trait]
        impl ToolRunner for SlowNever {
            async fn execute(&self, call: &ToolCallRequest) -> ark_common::Result<ToolOutput> {
                if call.call_id == "hanging" {
                    std::future::pending::<ark_common::Result<ToolOutput>>().await
                } else {
                    Ok(ToolOutput::ok("delivered"))
                }
            }
        }
        let mut stream = run_calls_streaming(
            Arc::new(SlowNever),
            vec![call("hanging", "read"), call("done", "read")],
        );
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        stream.abort();
        let mut got = Vec::new();
        while let Some(item) = stream.next().await {
            got.push((item.slot, item.execution.content));
        }
        assert_eq!(got.len(), 1, "only the settled call survives the abort");
        assert_eq!(got[0], (1, "delivered".to_string()));
    }

    #[tokio::test]
    async fn a_forwarder_panic_reports_through_join() {
        struct PanicsOnFirst;
        #[async_trait::async_trait]
        impl ToolRunner for PanicsOnFirst {
            async fn execute(&self, call: &ToolCallRequest) -> ark_common::Result<ToolOutput> {
                if call.call_id == "boom" {
                    panic!("tool exploded");
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                Ok(ToolOutput::ok("after"))
            }
        }
        let mut stream = run_calls_streaming(
            Arc::new(PanicsOnFirst),
            vec![call("boom", "read"), call("after", "read")],
        );
        let mut saw = Vec::new();
        while let Some(item) = stream.next().await {
            saw.push(item.execution.content);
        }
        let joined = stream.join().await;
        assert!(joined.is_err(), "the forwarder panic must surface via join");
        assert!(
            saw.iter().all(|content| content != "boom"),
            "the panicking call never produces a result"
        );
    }

    #[tokio::test]
    async fn dropping_the_stream_aborts_a_hanging_tool_future() {
        struct DropSignal(Arc<tokio::sync::Notify>);
        impl Drop for DropSignal {
            fn drop(&mut self) {
                self.0.notify_one();
            }
        }
        struct Hanging {
            started: Arc<tokio::sync::Notify>,
            dropped: Arc<tokio::sync::Notify>,
        }
        #[async_trait::async_trait]
        impl ToolRunner for Hanging {
            async fn execute(&self, _call: &ToolCallRequest) -> ark_common::Result<ToolOutput> {
                self.started.notify_one();
                let _signal = DropSignal(Arc::clone(&self.dropped));
                std::future::pending::<ark_common::Result<ToolOutput>>().await
            }
        }
        let started = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(tokio::sync::Notify::new());
        let stream = run_calls_streaming(
            Arc::new(Hanging {
                started: Arc::clone(&started),
                dropped: Arc::clone(&dropped),
            }),
            vec![call("hanging", "read")],
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
            .await
            .expect("tool starts");
        drop(stream);
        tokio::time::timeout(std::time::Duration::from_secs(1), dropped.notified())
            .await
            .expect("stream drop must abort the forwarder and drop the tool future");
    }

    #[tokio::test]
    async fn empty_batch_yields_nothing() {
        struct Nothing;
        #[async_trait::async_trait]
        impl ToolRunner for Nothing {
            async fn execute(&self, _call: &ToolCallRequest) -> ark_common::Result<ToolOutput> {
                Ok(ToolOutput::ok("unused"))
            }
        }
        let runner = Arc::new(Nothing);
        assert!(run_calls(runner, Vec::new()).await.is_empty());
    }
}
