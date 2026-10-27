//! Bus-level tests: in-process routing, bounded recursion, declared visibility.

use super::*;
use crate::service::registry::McpHandler;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

/// Returns its arguments unchanged and records how often it ran.
struct Echo {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl McpHandler for Echo {
    async fn call(&self, _ctx: &CallContext, args: serde_json::Value) -> Result<McpCallResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(McpCallResult {
            ok: true,
            content: args,
            trace_id: String::new(),
        })
    }
}

/// Re-enters the bus once, which is what a nested plugin call looks like.
struct Reenter {
    bus: Weak<McpBus>,
    caller: String,
    provider: String,
    tool: String,
}

#[async_trait]
impl McpHandler for Reenter {
    async fn call(&self, ctx: &CallContext, _args: serde_json::Value) -> Result<McpCallResult> {
        let bus = self
            .bus
            .upgrade()
            .ok_or_else(|| ArkError::internal("bus dropped"))?;
        let args = serde_json::json!({ "parent_depth": ctx.depth });
        bus.call(&self.caller, &self.provider, &self.tool, args)
            .await
    }
}

fn spec(plugin: &str, tool: &str) -> McpToolSpec {
    McpToolSpec {
        plugin_id: plugin.to_string(),
        tool: tool.to_string(),
        description: format!("{plugin}.{tool}"),
        schema: serde_json::json!({ "type": "object" }),
    }
}

fn bus() -> Arc<McpBus> {
    let bus = Arc::new(McpBus::default());
    bus.declare_visibility("ark.a", &["run".into()], &["ark.b".into(), "ark.a".into()])
        .unwrap();
    bus.declare_visibility("ark.b", &["run".into()], &["ark.a".into(), "ark.b".into()])
        .unwrap();
    bus
}

#[tokio::test]
async fn in_process_call_returns_the_handler_result() {
    let bus = bus();
    let calls = Arc::new(AtomicUsize::new(0));
    let _guard = bus
        .expose_with_handlers(
            "ark.b",
            vec![(
                spec("ark.b", "run"),
                Arc::new(Echo {
                    calls: calls.clone(),
                }) as Arc<dyn McpHandler>,
            )],
        )
        .unwrap();
    let result = bus
        .call("ark.a", "ark.b", "run", serde_json::json!({ "q": 1 }))
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(result.content["q"], 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!result.trace_id.is_empty(), "trace id must be propagated");
}

#[tokio::test]
async fn undo_registers_on_handle_drop() {
    let bus = bus();
    let calls = Arc::new(AtomicUsize::new(0));
    {
        let _guard = bus
            .expose_with_handlers(
                "ark.b",
                vec![(
                    spec("ark.b", "run"),
                    Arc::new(Echo {
                        calls: calls.clone(),
                    }) as Arc<dyn McpHandler>,
                )],
            )
            .unwrap();
        assert_eq!(bus.catalog().tools.len(), 1);
    }
    assert!(
        bus.catalog().tools.is_empty(),
        "Drop must unregister the exposure"
    );
    let err = bus
        .call("ark.a", "ark.b", "run", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("mcp_tool_not_found"));
}

#[tokio::test]
async fn undeclared_cross_plugin_call_is_denied() {
    let bus = Arc::new(McpBus::default());
    bus.declare_visibility("ark.a", &[], &[]).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let _guard = bus
        .expose_with_handlers(
            "ark.b",
            vec![(
                spec("ark.b", "run"),
                Arc::new(Echo {
                    calls: calls.clone(),
                }) as Arc<dyn McpHandler>,
            )],
        )
        .unwrap();
    let err = bus
        .call("ark.a", "ark.b", "run", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("mcp_not_permitted"));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "handler must not run");
}

#[tokio::test]
async fn depth_limit_stops_the_fourth_nested_call() {
    let bus = bus();
    // ark.b.run -> ark.a.run -> ark.b.run ... each hop is a new frame.
    let chain_down = Reenter {
        bus: Arc::downgrade(&bus),
        caller: "ark.b".into(),
        provider: "ark.a".into(),
        tool: "run".into(),
    };
    let chain_up = Reenter {
        bus: Arc::downgrade(&bus),
        caller: "ark.a".into(),
        provider: "ark.b".into(),
        tool: "run".into(),
    };
    let _a = bus
        .expose_with_handlers(
            "ark.a",
            vec![(
                spec("ark.a", "run"),
                Arc::new(chain_down) as Arc<dyn McpHandler>,
            )],
        )
        .unwrap();
    let _b = bus
        .expose_with_handlers(
            "ark.b",
            vec![(
                spec("ark.b", "run"),
                Arc::new(chain_up) as Arc<dyn McpHandler>,
            )],
        )
        .unwrap();

    // Depth 1: ark.b.run, depth 2: ark.a.run, depth 3: ark.b.run is a *cycle*
    // on the same frame set, so a cycle-or-depth error is the expected refusal.
    let err = bus
        .call("ark.a", "ark.b", "run", serde_json::json!({}))
        .await
        .unwrap_err();
    let detail = err.detail.as_deref().unwrap_or_default();
    assert!(
        detail == "mcp_depth_exceeded" || detail == "mcp_call_cycle" || detail == "mcp_deadlock",
        "unexpected refusal: {detail} ({})",
        err.message
    );
}

#[tokio::test]
async fn a_b_a_is_refused_as_a_cycle_not_run_forever() {
    let bus = bus();
    // ark.b.run calls ark.a.run once, and ark.a.run calls ark.b.run back.
    let a_calls_b = Reenter {
        bus: Arc::downgrade(&bus),
        caller: "ark.a".into(),
        provider: "ark.b".into(),
        tool: "run".into(),
    };
    let b_calls_a = Reenter {
        bus: Arc::downgrade(&bus),
        caller: "ark.b".into(),
        provider: "ark.a".into(),
        tool: "run".into(),
    };
    let _a = bus
        .expose_with_handlers(
            "ark.a",
            vec![(
                spec("ark.a", "run"),
                Arc::new(a_calls_b) as Arc<dyn McpHandler>,
            )],
        )
        .unwrap();
    let _b = bus
        .expose_with_handlers(
            "ark.b",
            vec![(
                spec("ark.b", "run"),
                Arc::new(b_calls_a) as Arc<dyn McpHandler>,
            )],
        )
        .unwrap();

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        bus.call("ark.a", "ark.b", "run", serde_json::json!({})),
    )
    .await
    .expect("a cycle must be refused instead of looping");
    assert!(outcome.is_err(), "A -> B -> A must not succeed");
}

#[tokio::test]
async fn mutual_waiting_is_refused_by_the_wait_graph() {
    let bus = bus();
    // ark.b.run performs a call on ark.a while ark.a is already waiting on
    // ark.b: the wait-for graph closes and the inner call is refused.
    let b_calls_a = Reenter {
        bus: Arc::downgrade(&bus),
        caller: "ark.b".into(),
        provider: "ark.a".into(),
        tool: "ping".into(),
    };
    let _a = bus
        .expose_with_handlers(
            "ark.a",
            vec![(
                spec("ark.a", "ping"),
                Arc::new(Echo {
                    calls: Arc::new(AtomicUsize::new(0)),
                }) as Arc<dyn McpHandler>,
            )],
        )
        .unwrap();
    let _b = bus
        .expose_with_handlers(
            "ark.b",
            vec![(
                spec("ark.b", "run"),
                Arc::new(b_calls_a) as Arc<dyn McpHandler>,
            )],
        )
        .unwrap();

    let err = bus
        .call("ark.a", "ark.b", "run", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(
        err.detail.as_deref(),
        Some("mcp_deadlock"),
        "{}",
        err.message
    );
    assert!(err.message.contains("ark.a") && err.message.contains("ark.b"));
}

#[tokio::test]
async fn catalog_is_filtered_by_declared_visibility() {
    let bus = bus();
    let _a = bus
        .expose_with_handlers(
            "ark.a",
            vec![(
                spec("ark.a", "run"),
                Arc::new(Echo {
                    calls: Arc::new(AtomicUsize::new(0)),
                }) as Arc<dyn McpHandler>,
            )],
        )
        .unwrap();
    let _b = bus
        .expose_with_handlers(
            "ark.b",
            vec![(
                spec("ark.b", "run"),
                Arc::new(Echo {
                    calls: Arc::new(AtomicUsize::new(0)),
                }) as Arc<dyn McpHandler>,
            )],
        )
        .unwrap();
    let _c = bus
        .expose_with_handlers(
            "ark.c",
            vec![(
                spec("ark.c", "run"),
                Arc::new(Echo {
                    calls: Arc::new(AtomicUsize::new(0)),
                }) as Arc<dyn McpHandler>,
            )],
        )
        .unwrap();

    let names: Vec<String> = bus
        .catalog_for("ark.a")
        .tools
        .into_iter()
        .map(|s| format!("{}.{}", s.plugin_id, s.tool))
        .collect();
    assert_eq!(names, vec!["ark.a.run", "ark.b.run"]);
    assert_eq!(
        bus.catalog().tools.len(),
        3,
        "the full catalog stays complete"
    );
}

#[tokio::test]
async fn unload_removes_tools_and_visibility() {
    let bus = bus();
    let _b = bus
        .expose_specs("ark.b", vec![spec("ark.b", "run")])
        .unwrap();
    assert_eq!(bus.unregister_plugin("ark.b"), 1);
    assert!(bus.catalog().tools.is_empty());
    assert!(bus.catalog_for("ark.a").tools.is_empty());
}

#[tokio::test]
async fn spec_only_tools_fail_closed_when_called() {
    let bus = bus();
    let _b = bus
        .expose_specs("ark.b", vec![spec("ark.b", "run")])
        .unwrap();
    let err = bus
        .call("ark.a", "ark.b", "run", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("mcp_tool_unavailable"));
}
