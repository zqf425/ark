//! TypeScript generation for the wire contract (design §6.1 / §6.2).
//!
//! The renderer must not hand-maintain a second copy of these shapes: a single
//! field renamed on one side would silently blank a panel, and a new
//! `ArkErrorCode` would fall through the UI's `switch` with no compiler help.
//!
//! `ts-rs` derives are on the types themselves, so the Rust definition is the
//! only source. This module is the *executor*: it walks every root type and
//! writes the bindings into `web/src/generated/`.
//!
//! Run it with:
//!
//! ```text
//! cargo test -p ark-contract --lib ts_export
//! ```
//!
//! `web/scripts/check-contracts.mjs` then asserts the checked-in output still
//! matches what the Rust types produce, which is what makes drift a CI failure
//! instead of a runtime surprise.

use ts_rs::TS;

use crate::item::{ItemKind, ToolPayload};
use crate::{
    ApplyResult, AttachmentDiagnostics, AttachmentEntry, AttachmentUploadRequest,
    AttachmentUploadResponse, ChildCompletionNotice, ChildRunActionResult, ChildRunAggregates,
    ChildRunEventsResponse, ChildRunListResponse, ChildRunSummary, ConfigPatch,
    CreateMemoryRequest, CreateThreadRequest, EventKind, FieldPath, FilePreview, GoalRequest,
    HealthResponse, ItemStatus, LlmRound, LlmRoundError, LlmRoundsResponse, McpServerEntry,
    McpServerListResponse, McpServerTestResponse, McpServerView, McpTransportKind,
    MemoryDiagnosticsResponse, MemoryEntry, MemoryListResponse, PendingApprovalView,
    PendingUserInputView, PlanView, ProviderEntry, ProviderListResponse,
    ProviderModelCatalogConnection, ProviderModelCatalogRequest, ProviderModelCatalogResponse,
    ProviderModelSummary, ProviderTestRequest, ProviderTestResponse, ProviderView,
    ResolveApprovalRequest, ResolveUserInputRequest, ResumeChildRequest, RewindRequest,
    RewindResponse, RuntimeConfigView, RuntimeEvent, RuntimeInfo, SkillDiagnostics,
    SkillLoadWarning, SkillSource, SkillSummary, SteerChildRequest, SubagentSettings,
    SwitchWorkspaceRequest, TerminalEntriesResponse, TerminalEntry, ThreadDetail, ThreadItem,
    ThreadListQuery, ThreadRuntimeSettings, ThreadRuntimeSnapshot, ThreadSkillsResponse,
    ThreadSummary, ThreadUsageResponse, TodoEntry, TodoStatus, TodosRequest, TuningSettings,
    TurnRequest, TurnSummary, UpdateMemoryRequest, UpdateThreadRequest, UsageResponse,
    WorkspaceChange, WorkspaceChangesResponse, WorkspaceDescriptor, WorkspaceDiff,
    WorkspaceDiffFile, WorkspaceStatus, WorkspaceView,
};
use ark_common::{
    ArkErrorCode, ArkErrorPayload, AttachmentId, ChildId, ItemId, ThreadId, TurnId, Usage,
};

/// Directory the bindings are written to, relative to this crate.
pub const OUT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/src/generated");

/// Every root the renderer consumes.
///
/// Order is irrelevant to correctness (each type lands in its own file), but
/// keeping it grouped by area makes a missing entry obvious when a new DTO is
/// added.
fn export_roots(dir: &std::path::Path) -> Result<(), ts_rs::ExportError> {
    // ---- shared scalars and error taxonomy ----
    ThreadId::export_all_to(dir)?;
    TurnId::export_all_to(dir)?;
    ItemId::export_all_to(dir)?;
    ChildId::export_all_to(dir)?;
    AttachmentId::export_all_to(dir)?;
    Usage::export_all_to(dir)?;
    ArkErrorCode::export_all_to(dir)?;
    ArkErrorPayload::export_all_to(dir)?;

    // ---- thread and turn surface ----
    ThreadSummary::export_all_to(dir)?;
    ThreadListQuery::export_all_to(dir)?;
    CreateThreadRequest::export_all_to(dir)?;
    UpdateThreadRequest::export_all_to(dir)?;
    ThreadDetail::export_all_to(dir)?;
    ThreadRuntimeSnapshot::export_all_to(dir)?;
    PendingApprovalView::export_all_to(dir)?;
    PendingUserInputView::export_all_to(dir)?;
    ThreadRuntimeSettings::export_all_to(dir)?;
    TurnRequest::export_all_to(dir)?;
    RewindRequest::export_all_to(dir)?;
    RewindResponse::export_all_to(dir)?;
    TurnSummary::export_all_to(dir)?;
    ResolveApprovalRequest::export_all_to(dir)?;
    ResolveUserInputRequest::export_all_to(dir)?;
    PlanView::export_all_to(dir)?;
    GoalRequest::export_all_to(dir)?;
    TodosRequest::export_all_to(dir)?;
    TodoEntry::export_all_to(dir)?;
    TodoStatus::export_all_to(dir)?;
    WorkspaceChangesResponse::export_all_to(dir)?;
    WorkspaceChange::export_all_to(dir)?;
    WorkspaceDescriptor::export_all_to(dir)?;
    WorkspaceView::export_all_to(dir)?;
    SwitchWorkspaceRequest::export_all_to(dir)?;
    WorkspaceDiffFile::export_all_to(dir)?;
    WorkspaceDiff::export_all_to(dir)?;
    FilePreview::export_all_to(dir)?;
    TerminalEntriesResponse::export_all_to(dir)?;
    TerminalEntry::export_all_to(dir)?;
    UsageResponse::export_all_to(dir)?;
    ThreadUsageResponse::export_all_to(dir)?;
    LlmRound::export_all_to(dir)?;
    LlmRoundError::export_all_to(dir)?;
    LlmRoundsResponse::export_all_to(dir)?;
    HealthResponse::export_all_to(dir)?;
    RuntimeInfo::export_all_to(dir)?;
    AttachmentEntry::export_all_to(dir)?;
    AttachmentUploadRequest::export_all_to(dir)?;
    AttachmentUploadResponse::export_all_to(dir)?;
    AttachmentDiagnostics::export_all_to(dir)?;

    // ---- long-term memory ----
    MemoryEntry::export_all_to(dir)?;
    CreateMemoryRequest::export_all_to(dir)?;
    UpdateMemoryRequest::export_all_to(dir)?;
    MemoryListResponse::export_all_to(dir)?;
    MemoryDiagnosticsResponse::export_all_to(dir)?;

    // ---- render items and events ----
    ItemKind::export_all_to(dir)?;
    ItemStatus::export_all_to(dir)?;
    ToolPayload::export_all_to(dir)?;
    ThreadItem::export_all_to(dir)?;
    ChildCompletionNotice::export_all_to(dir)?;
    EventKind::export_all_to(dir)?;
    RuntimeEvent::export_all_to(dir)?;

    // ---- child runs ----
    ChildRunSummary::export_all_to(dir)?;
    ChildRunAggregates::export_all_to(dir)?;
    ChildRunListResponse::export_all_to(dir)?;
    ChildRunActionResult::export_all_to(dir)?;
    ChildRunEventsResponse::export_all_to(dir)?;
    SteerChildRequest::export_all_to(dir)?;
    ResumeChildRequest::export_all_to(dir)?;

    // ---- settings surface ----
    RuntimeConfigView::export_all_to(dir)?;
    ConfigPatch::export_all_to(dir)?;
    ApplyResult::export_all_to(dir)?;
    SubagentSettings::export_all_to(dir)?;
    TuningSettings::export_all_to(dir)?;
    ProviderEntry::export_all_to(dir)?;
    ProviderView::export_all_to(dir)?;
    ProviderListResponse::export_all_to(dir)?;
    ProviderModelCatalogConnection::export_all_to(dir)?;
    ProviderModelCatalogRequest::export_all_to(dir)?;
    ProviderModelSummary::export_all_to(dir)?;
    ProviderModelCatalogResponse::export_all_to(dir)?;
    ProviderTestRequest::export_all_to(dir)?;
    ProviderTestResponse::export_all_to(dir)?;
    WorkspaceStatus::export_all_to(dir)?;
    McpTransportKind::export_all_to(dir)?;
    McpServerEntry::export_all_to(dir)?;
    McpServerView::export_all_to(dir)?;
    McpServerListResponse::export_all_to(dir)?;
    McpServerTestResponse::export_all_to(dir)?;
    SkillSummary::export_all_to(dir)?;
    SkillSource::export_all_to(dir)?;
    SkillLoadWarning::export_all_to(dir)?;
    SkillDiagnostics::export_all_to(dir)?;
    ThreadSkillsResponse::export_all_to(dir)?;
    FieldPath::export_all_to(dir)?;

    Ok(())
}

/// Write every binding, then the barrel the renderer imports.
fn generate(dir: &std::path::Path) -> Result<usize, ts_rs::ExportError> {
    std::fs::create_dir_all(dir)
        .map_err(|_| ts_rs::ExportError::CannotBeExported("cannot create the output directory"))?;
    export_roots(dir)?;
    let files = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    std::fs::write(dir.join("index.ts"), barrel())
        .map_err(|_| ts_rs::ExportError::CannotBeExported("cannot write the generated barrel"))?;
    Ok(files)
}

/// Barrel that re-exports every generated type from one module path.
///
/// Written by the generator so adding a type cannot leave the renderer without
/// an import path.
fn barrel() -> String {
    let mut out = String::from(
        "// @generated by `cargo test -p ark-contract --lib ts_export`.\n\
         // Do not edit: change the Rust type and re-run the generator.\n",
    );
    for name in ROOT_NAMES {
        out.push_str(&format!("export type {{ {name} }} from \"./{name}\";\n"));
    }
    out
}

/// Names in the same order as [`export_roots`].
const ROOT_NAMES: &[&str] = &[
    "ThreadId",
    "TurnId",
    "ItemId",
    "ChildId",
    "AttachmentId",
    "Usage",
    "ArkErrorCode",
    "ArkErrorPayload",
    "ThreadSummary",
    "ThreadListQuery",
    "ThreadUsageResponse",
    "LlmRound",
    "LlmRoundError",
    "LlmRoundsResponse",
    "CreateThreadRequest",
    "UpdateThreadRequest",
    "ThreadDetail",
    "ThreadRuntimeSnapshot",
    "PendingApprovalView",
    "PendingUserInputView",
    "ThreadRuntimeSettings",
    "TurnRequest",
    "RewindRequest",
    "RewindResponse",
    "TurnSummary",
    "ResolveApprovalRequest",
    "ResolveUserInputRequest",
    "PlanView",
    "GoalRequest",
    "TodosRequest",
    "TodoEntry",
    "TodoStatus",
    "WorkspaceChangesResponse",
    "WorkspaceChange",
    "WorkspaceDescriptor",
    "WorkspaceView",
    "SwitchWorkspaceRequest",
    "WorkspaceDiffFile",
    "WorkspaceDiff",
    "FilePreview",
    "TerminalEntriesResponse",
    "TerminalEntry",
    "UsageResponse",
    "HealthResponse",
    "RuntimeInfo",
    "MemoryEntry",
    "CreateMemoryRequest",
    "UpdateMemoryRequest",
    "MemoryListResponse",
    "MemoryDiagnosticsResponse",
    "AttachmentEntry",
    "AttachmentUploadRequest",
    "AttachmentUploadResponse",
    "AttachmentDiagnostics",
    "ItemKind",
    "ItemStatus",
    "ToolPayload",
    "ThreadItem",
    "ChildCompletionNotice",
    "EventKind",
    "RuntimeEvent",
    "ChildRunSummary",
    "ChildRunAggregates",
    "ChildRunListResponse",
    "ChildRunActionResult",
    "ChildRunEventsResponse",
    "SteerChildRequest",
    "ResumeChildRequest",
    "RuntimeConfigView",
    "ConfigPatch",
    "ApplyResult",
    "SubagentSettings",
    "TuningSettings",
    "ProviderEntry",
    "ProviderView",
    "ProviderListResponse",
    "ProviderModelCatalogConnection",
    "ProviderModelCatalogRequest",
    "ProviderModelSummary",
    "ProviderModelCatalogResponse",
    "ProviderTestRequest",
    "ProviderTestResponse",
    "WorkspaceStatus",
    "McpTransportKind",
    "McpServerEntry",
    "McpServerView",
    "McpServerListResponse",
    "McpServerTestResponse",
    "SkillSummary",
    "SkillSource",
    "SkillLoadWarning",
    "SkillDiagnostics",
    "ThreadSkillsResponse",
    "FieldPath",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Regenerate the bindings, then assert what was written.
    ///
    /// Deliberately the only test that writes into the repository: the output is
    /// checked in so the renderer can typecheck without a Rust toolchain, and CI
    /// re-runs this and fails on a diff.
    ///
    /// Generation and its invariants live in **one** test on purpose. Splitting
    /// them made the read-only test race the writer: `cargo test` runs tests in
    /// parallel, so it could observe a half-written directory.
    #[test]
    fn regenerate_typescript_bindings() {
        let dir = std::path::Path::new(OUT_DIR);
        let files = generate(dir).expect("bindings must be generatable");
        assert!(
            files >= ROOT_NAMES.len(),
            "expected at least one file per root type, wrote {files}"
        );
        assert!(
            dir.join("index.ts").is_file(),
            "the barrel must exist after generation"
        );

        // Every name in the barrel must have a file behind it, or the renderer
        // would fail to resolve an import the generator itself emitted.
        for name in ROOT_NAMES {
            assert!(
                dir.join(format!("{name}.ts")).is_file(),
                "`{name}` is listed in the barrel but was never generated"
            );
        }

        // Ids stay strings: the renderer treats them as opaque, and a struct
        // shape here would leak the UUID newtype into the wire contract.
        let thread_id = std::fs::read_to_string(dir.join("ThreadId.ts")).unwrap();
        assert!(thread_id.contains("type ThreadId = string"), "{thread_id}");
        let thread = std::fs::read_to_string(dir.join("ThreadSummary.ts")).unwrap();
        assert!(thread.contains("thread_id: ThreadId"), "{thread}");

        // Counters are `number`, never `bigint`: the wire format is JSON, and a
        // `bigint` would be unrepresentable in `JSON.parse`.
        let usage = std::fs::read_to_string(dir.join("Usage.ts")).unwrap();
        assert!(!usage.contains("bigint"), "{usage}");
        assert!(usage.contains("prompt_tokens: number"), "{usage}");

        // The event union must keep its `kind` discriminator.
        let event = std::fs::read_to_string(dir.join("RuntimeEvent.ts")).unwrap();
        assert!(event.contains("export type RuntimeEvent"), "{event}");
        assert!(event.contains("\"kind\""), "{event}");
    }
}
