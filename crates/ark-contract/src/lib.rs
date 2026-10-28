//! ark-contract: the single source of truth for every cross-process payload.
//!
//! Design §6.1 splits shared material into two classes. This crate only owns
//! the *data contract* (structures, enums, error codes). Executable logic such
//! as path normalization lives in `ark-common` and is never re-implemented by a
//! client. Nothing here touches I/O, so the renderer-facing shapes can be
//! code-generated from these types without dragging runtime code along.

pub mod base;
pub mod capability;
pub mod config;
pub mod dto;
pub mod event;
pub mod item;
/// TypeScript binding generation (design §6.1).
///
/// Test-only: it writes into `web/src/generated/`, and the checked-in output is
/// what keeps the renderer and this crate from drifting apart.
#[cfg(test)]
pub mod ts_export;

pub use base::{
    McpServerEntry, McpServerListResponse, McpServerTestResponse, McpServerView, McpTransportKind,
    ProviderEntry, ProviderListResponse, ProviderModelCatalogConnection,
    ProviderModelCatalogRequest, ProviderModelCatalogResponse, ProviderModelSummary,
    ProviderTestRequest, ProviderTestResponse, ProviderView, SkillActivation, SkillDiagnostics,
    SkillLoadWarning, SkillManifest, SkillSource, SkillSummary, SkillTriggers, SkillTurnResolution,
    ThreadSkillsResponse, WorkspaceStatus,
};
pub use capability::{Capability, PageId, PageSpec, PanelSlot, PanelSpec, Placement};
pub use config::{
    ApplyResult, ConfigPatch, FieldPath, ReloadClass, RuntimeConfigView, SubagentSettings,
    TuningSettings, SUBAGENT_CONCURRENCY_MAX, SUBAGENT_CONCURRENCY_MIN,
};
pub use dto::{
    ChildRunActionResult, ChildRunAggregates, ChildRunEventsResponse, ChildRunListResponse,
    ChildRunSummary, CreateThreadRequest, FilePreview, GoalRequest, HealthResponse,
    PendingApprovalView, PendingUserInputView, PlanView, ResolveApprovalRequest,
    ResolveUserInputRequest, ResumeChildRequest, RuntimeInfo, SteerChildRequest,
    SwitchWorkspaceRequest, TerminalEntriesResponse, TerminalEntry, ThreadDetail,
    ThreadRuntimeSnapshot, ThreadSummary, TodoEntry, TodoStatus, TodosRequest, TurnRequest,
    TurnSummary, UpdateThreadRequest, UsageResponse, WorkspaceChange, WorkspaceChangesResponse,
    WorkspaceDescriptor, WorkspaceDiff, WorkspaceDiffFile, WorkspaceView,
};
pub use event::{ChildCompletionNotice, EventKind, RuntimeEvent};
pub use item::{ItemStatus, ThreadItem};
