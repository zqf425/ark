//! ark-contract: the single source of truth for every cross-process payload.
//!
//! Design §6.1 splits shared material into two classes. This crate only owns
//! the *data contract* (structures, enums, error codes). Executable logic such
//! as path normalization lives in `ark-common` and is never re-implemented by a
//! client. Nothing here touches I/O, so the renderer-facing shapes can be
//! code-generated from these types without dragging runtime code along.

pub mod attachment;
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

pub use attachment::{
    AttachmentAssociationRequest, AttachmentAssociationResponse, AttachmentDiagnostics,
    AttachmentEntry, AttachmentUploadRequest, AttachmentUploadResponse,
};
pub use base::{
    CreateMemoryRequest, McpServerEntry, McpServerListResponse, McpServerTestResponse,
    McpServerView, McpTransportKind, MemoryDiagnosticsResponse, MemoryEntry, MemoryListResponse,
    ProviderEntry, ProviderListResponse, ProviderModelCatalogConnection,
    ProviderModelCatalogRequest, ProviderModelCatalogResponse, ProviderModelSummary,
    ProviderTestRequest, ProviderTestResponse, ProviderView, SkillActivation, SkillDiagnostics,
    SkillLoadWarning, SkillManifest, SkillSource, SkillSummary, SkillTriggers, SkillTurnResolution,
    ThreadSkillsResponse, UpdateMemoryRequest, WorkspaceStatus, WorkspaceValidateResponse,
};
pub use capability::{
    CapabilitiesResponse, Capability, PageId, PageSpec, PanelEntry, PanelSlot, PanelSpec,
    Placement, UiCatalog,
};
pub use config::{
    ApplyResult, ConfigPatch, FieldPath, ReloadClass, RuntimeConfigView, SubagentSettings,
    TuningSettings, SUBAGENT_CONCURRENCY_MAX, SUBAGENT_CONCURRENCY_MIN,
};
pub use dto::{
    ChildRunActionResult, ChildRunAggregates, ChildRunEventsResponse, ChildRunListResponse,
    ChildRunSummary, CreateThreadRequest, FilePreview, GoalRequest, HealthResponse, LlmRound,
    LlmRoundError, LlmRoundsResponse, PendingApprovalView, PendingUserInputView, PlanView,
    ResolveApprovalRequest, ResolveUserInputRequest, ResumeChildRequest, ReviewRequest,
    RewindRequest, RewindResponse, RuntimeInfo, SessionResumeResponse, SteerChildRequest,
    SwitchWorkspaceRequest, TerminalEntriesResponse, TerminalEntry, ThreadDetail, ThreadExport,
    ThreadListQuery, ThreadRuntimeSettings, ThreadRuntimeSnapshot, ThreadSummary,
    ThreadUsageResponse, TodoEntry, TodoStatus, TodosRequest, TurnRequest, TurnSummary,
    UpdateThreadRequest, UsageResponse, WorkspaceChange, WorkspaceChangesResponse,
    WorkspaceDescriptor, WorkspaceDiff, WorkspaceDiffFile, WorkspaceView, LLM_ROUNDS_DEFAULT_LIMIT,
    LLM_ROUNDS_MAX_LIMIT, LLM_ROUNDS_MAX_THREADS, MAX_TURN_TEXT_BYTES,
    THREAD_EXPORT_FORMAT_VERSION, THREAD_PAGE_DEFAULT_LIMIT, THREAD_PAGE_MAX_LIMIT,
};
pub use event::{ChildCompletionNotice, EventKind, RuntimeEvent};
pub use item::{ItemStatus, ThreadItem};
