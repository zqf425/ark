//! Concurrent tool execution for one batch (design §7.1 `DispatchingTools`).
//!
//! Batching decisions belong to `policy::parallel`; this module only runs what
//! it was given and measures each call. Kept separate so the reactor stays a
//! dispatcher and this stays a tiny, testable helper.

use std::sync::Arc;
use std::time::Instant;

use crate::domain::tool::{ToolCallRequest, ToolExecution};
use crate::exec::tool_runner::ToolRunner;
use crate::service::reactor::execution_of;
use tokio::sync::Semaphore;

/// Run every call in `calls` concurrently, preserving input order.
pub async fn run_calls(
    tools: Arc<dyn ToolRunner>,
    calls: Vec<ToolCallRequest>,
) -> Vec<ToolExecution> {
    let futures = calls.into_iter().map(|call| {
        let tools = Arc::clone(&tools);
        async move {
            let start = Instant::now();
            let output = tools.execute(&call).await;
            let duration_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            execution_of(&call, output, duration_ms)
        }
    });
    futures::future::join_all(futures).await
}

/// Run every call in `calls` concurrently with concurrency limits.
pub async fn run_calls_with_limit(
    tools: Arc<dyn ToolRunner>,
    calls: Vec<ToolCallRequest>,
    semaphore: Arc<Semaphore>,
) -> Vec<ToolExecution> {
    let futures = calls.into_iter().map(|call| {
        let tools = Arc::clone(&tools);
        let semaphore = Arc::clone(&semaphore);
        async move {
            let _permit = semaphore.acquire().await.unwrap();
            let start = Instant::now();
            let output = tools.execute(&call).await;
            let duration_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            execution_of(&call, output, duration_ms)
        }
    });
    futures::future::join_all(futures).await
}

/// Run calls with priority-based scheduling.
#[derive(Debug, Clone)]
pub struct ToolCallWithPriority {
    pub call: ToolCallRequest,
    pub priority: u8, // 0-255, higher is more important
}

pub async fn run_calls_with_priority(
    tools: Arc<dyn ToolRunner>,
    calls: Vec<ToolCallWithPriority>,
) -> Vec<ToolExecution> {
    // Group by priority threshold
    let mut high_priority = Vec::new();
    let mut normal_priority = Vec::new();
    
    for call_with_priority in calls {
        if call_with_priority.priority > 128 {
            high_priority.push(call_with_priority.call);
        } else {
            normal_priority.push(call_with_priority.call);
        }
    }
    
    let high_results = run_calls(Arc::clone(&tools), high_priority).await;
    let normal_results = run_calls(tools, normal_priority).await;
    
    [high_results, normal_results].concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::tool_runner::MapToolRunner;
    use ark_ports::ToolOutput;

    fn call(id: &str, tool: &str) -> ToolCallRequest {
        ToolCallRequest::new(id, tool, serde_json::json!({ "path": "a" }))
    }

    #[tokio::test]
    async fn results_preserve_input_order() {
        let runner = Arc::new(
            MapToolRunner::new()
                .with("read", ToolOutput::ok("body"))
                .with("write", ToolOutput::err("denied")),
        );
        let calls = vec![call("c1", "read"), call("c2", "write")];
        let executions = run_calls(runner, calls).await;
        assert_eq!(executions.len(), 2);
        assert_eq!(executions[0].call_id, "c1");
        assert!(executions[0].ok);
        assert_eq!(executions[1].call_id, "c2");
        assert!(!executions[1].ok);
    }

    #[tokio::test]
    async fn empty_batch_yields_nothing() {
        let runner = Arc::new(MapToolRunner::new());
        assert!(run_calls(runner, Vec::new()).await.is_empty());
    }
}
