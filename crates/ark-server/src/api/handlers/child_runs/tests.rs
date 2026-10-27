//! `child-runs` handler tests.
//!
//! Only the pure translation helpers are exercised here: folding the
//! append-only run log, folding aggregates, and the query validation. The
//! runtime behaviour behind the ports is covered by `ark-delegation`, and the
//! store behaviour by `ark-store`; duplicating those here would test the fakes
//! rather than the boundary.

use super::*;
use ark_common::{ChildId, ThreadId, TurnId, Usage};
use chrono::Utc;

fn record(parent: ThreadId, child: ChildId, status: &str, minutes_ago: i64) -> ChildRunRecord {
    let mut record = ChildRunRecord::queued(
        child,
        parent,
        TurnId::new(),
        "scan",
        Utc::now() - chrono::Duration::minutes(minutes_ago),
    );
    record.status = status.to_string();
    record
}

#[test]
fn a_child_id_must_be_a_uuid() {
    let err = parse_child("ch_1").unwrap_err();
    assert_eq!(err.0.code, ArkErrorCode::ValidationError);
    assert!(err.0.message.contains("not a valid child id"));
}

#[test]
fn a_valid_child_id_round_trips() {
    let child = ChildId::new();
    assert_eq!(parse_child(&child.to_string()).unwrap(), child);
}

#[test]
fn a_missing_parent_is_a_validation_error_not_a_guess() {
    // Ownership is always re-checked against a parent, so an unscoped request
    // must fail loudly instead of reading every thread's children.
    let err = require_parent(&ChildRunsQuery::default()).unwrap_err();
    assert_eq!(err.0.code, ArkErrorCode::ValidationError);
    assert!(err.0.message.contains("parentThreadId is required"));
}

#[test]
fn a_present_parent_is_parsed() {
    let parent = ThreadId::new();
    let query = ChildRunsQuery {
        thread_id: None,
        parent_thread_id: Some(parent.to_string()),
    };
    assert_eq!(require_parent(&query).unwrap(), parent);
}

#[test]
fn folding_keeps_the_latest_row_per_child() {
    // `children/runs.jsonl` is append-only: queued, running and completed rows
    // all exist for one child, and the panel must show the last one.
    let parent = ThreadId::new();
    let child = ChildId::new();
    let folded = fold_latest(vec![
        record(parent, child, child_status::QUEUED, 10),
        record(parent, child, child_status::RUNNING, 5),
        record(parent, child, child_status::COMPLETED, 1),
    ]);
    assert_eq!(folded.len(), 1);
    assert_eq!(folded[0].status, child_status::COMPLETED);
}

#[test]
fn folding_orders_children_newest_first_with_a_stable_tiebreak() {
    let parent = ThreadId::new();
    let older = ChildId::new();
    let newer = ChildId::new();
    let folded = fold_latest(vec![
        record(parent, older, child_status::COMPLETED, 30),
        record(parent, newer, child_status::RUNNING, 1),
    ]);
    assert_eq!(folded[0].child_id, newer);
    assert_eq!(folded[1].child_id, older);

    // Same timestamp: the id decides, so two runs can never swap places between
    // refreshes and make the list flicker.
    let same = Utc::now();
    let mut a = record(parent, older, child_status::COMPLETED, 0);
    let mut b = record(parent, newer, child_status::COMPLETED, 0);
    a.created_at = same;
    b.created_at = same;
    let again = fold_latest(vec![b.clone(), a.clone()]);
    let reversed = fold_latest(vec![a, b]);
    assert_eq!(
        again.iter().map(|r| r.child_id).collect::<Vec<_>>(),
        reversed.iter().map(|r| r.child_id).collect::<Vec<_>>()
    );
}

#[test]
fn aggregates_count_every_terminal_status() {
    let parent = ThreadId::new();
    let mut failed = record(parent, ChildId::new(), child_status::FAILED, 5);
    failed.tool_invocations = Some(2);
    let mut done = record(parent, ChildId::new(), child_status::COMPLETED, 4);
    done.tool_invocations = Some(3);
    done.usage = Some(Usage::new(10, 5));
    let aggregates = aggregates_of(&[
        record(parent, ChildId::new(), child_status::QUEUED, 3),
        record(parent, ChildId::new(), child_status::RUNNING, 2),
        record(parent, ChildId::new(), child_status::ABORTED, 1),
        failed,
        done,
    ]);
    assert_eq!(aggregates.queued, 1);
    assert_eq!(aggregates.running, 1);
    assert_eq!(aggregates.completed, 1);
    assert_eq!(aggregates.failed, 1);
    assert_eq!(aggregates.aborted, 1);
    assert_eq!(aggregates.tool_invocations, 5);
    assert_eq!(aggregates.usage.total_tokens, 15);
}

#[test]
fn a_child_that_never_reached_a_model_reports_no_tokens() {
    // A queued child has no usage sample. Summing a zero would look like a real
    // measurement of a free run, which is exactly what the design forbids.
    let parent = ThreadId::new();
    let aggregates = aggregates_of(&[record(parent, ChildId::new(), child_status::QUEUED, 1)]);
    assert_eq!(aggregates.usage.total_tokens, 0);
    assert_eq!(aggregates.usage.cache_hit_rate, None);
}

#[test]
fn an_unknown_status_is_counted_nowhere_but_still_counts_its_tools() {
    // Forward compatibility: a future status must not crash the panel, and its
    // tool count is still real telemetry.
    let parent = ThreadId::new();
    let mut record = record(parent, ChildId::new(), "paused", 1);
    record.tool_invocations = Some(4);
    let aggregates = aggregates_of(&[record]);
    assert_eq!(aggregates.queued + aggregates.running, 0);
    assert_eq!(aggregates.tool_invocations, 4);
}

#[test]
fn a_summary_payload_carries_the_child_identity_and_real_usage() {
    let parent = ThreadId::new();
    let child = ChildId::new();
    let mut source = record(parent, child, child_status::COMPLETED, 1);
    source.summary = Some("done".into());
    source.tool_invocations = Some(7);
    source.usage = Some(Usage::new(20, 10));
    source.resume_count = Some(2);
    let summary = summary_of_record(&source);
    assert_eq!(summary.id, child);
    assert_eq!(summary.parent_thread_id, parent);
    assert_eq!(summary.status, child_status::COMPLETED);
    assert_eq!(summary.summary.as_deref(), Some("done"));
    assert_eq!(summary.tool_invocations, Some(7));
    assert_eq!(summary.resume_count, Some(2));
    assert_eq!(summary.usage.map(|u| u.total_tokens), Some(30));
}

#[test]
fn the_summary_serializes_to_the_field_names_the_renderer_parses() {
    // The renderer reads `ChildRunRecordJson` (camelCase). A snake_case payload
    // would silently blank the sub-agent panel, so the wire shape is asserted.
    let parent = ThreadId::new();
    let child = ChildId::new();
    let summary = summary_of_record(&record(parent, child, child_status::COMPLETED, 1));
    let json = serde_json::to_value(&summary).unwrap();
    assert!(json.get("parentThreadId").is_some(), "{json}");
    assert!(json.get("parentTurnId").is_some(), "{json}");
    assert!(json.get("createdAt").is_some(), "{json}");
    assert_eq!(json["id"], serde_json::json!(child.to_string()));
    assert!(json.get("parent_thread_id").is_none());
}
