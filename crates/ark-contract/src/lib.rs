//! ark-contract: the single source of truth for every cross-process payload.
//!
//! Design §6.1 splits shared material into two classes. This crate only owns
//! the *data contract* (structures, enums, error codes). Executable logic such
//! as path normalization lives in `ark-common` and is never re-implemented by a
//! client. Nothing here touches I/O, so the renderer-facing shapes can be
//! code-generated from these types without dragging runtime code along.

#![deny(clippy::all)]

pub mod attachment;
pub mod base;
pub mod capability;
pub mod config;
pub mod dto;
pub mod event;
pub mod im;
pub mod item;
pub mod schedule;
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
    CreateMemoryRequest, HookAuditView, McpServerEntry, McpServerListResponse,
    McpServerTestResponse, McpServerView, McpTransportKind, MemoryDiagnosticsResponse, MemoryEntry,
    MemoryListResponse, PluginDiagnosticView, PluginEnableRequest, PluginEntry, PluginHostReport,
    PluginProvisionView, ProviderEntry, ProviderListResponse, ProviderModelCatalogConnection,
    ProviderModelCatalogRequest, ProviderModelCatalogResponse, ProviderModelInventoryItem,
    ProviderModelSummary, ProviderTestRequest, ProviderTestResponse, ProviderView, SkillActivation,
    SkillDiagnostics, SkillLoadWarning, SkillManifest, SkillSource, SkillSummary, SkillTriggers,
    SkillTurnResolution, ThreadSkillsResponse, UpdateMemoryRequest, WorkspaceStatus,
    WorkspaceValidateResponse,
};
pub use capability::{
    CapabilitiesResponse, Capability, PageId, PageSpec, PanelEntry, PanelSlot, PanelSpec,
    Placement, UiCatalog,
};
pub use config::{
    ApplyResult, ConfigPatch, FieldPath, HookRule, HookSettings, ImSettings, ReloadClass,
    RuntimeConfigView, ScheduleSettings, SkillsSettings, SubagentSettings, TuningSettings,
    SKILLS_EXTRA_DIRS_MAX, SKILLS_EXTRA_DIR_MAX_CHARS, SUBAGENT_CONCURRENCY_MAX,
    SUBAGENT_CONCURRENCY_MIN,
};
pub use dto::{
    BulkDeleteSkipped, BulkDeleteThreadsResponse, ChildRunActionResult, ChildRunAggregates,
    ChildRunEventsResponse, ChildRunListResponse, ChildRunSummary, CreateThreadRequest,
    FilePreview, FolderPickResponse, GoalRequest, HealthResponse, LlmRound, LlmRoundError,
    LlmRoundsResponse, PendingApprovalView, PendingUserInputView, PlanView, ResolveApprovalRequest,
    ResolveUserInputRequest, ResumeChildRequest, ReviewRequest, RewindRequest, RewindResponse,
    RuntimeInfo, SessionResumeResponse, SteerChildRequest, SwitchWorkspaceRequest,
    TerminalEntriesResponse, TerminalEntry, ThreadDetail, ThreadExport, ThreadListQuery,
    ThreadRuntimeSettings, ThreadRuntimeSnapshot, ThreadSummary, ThreadUsageResponse, TodoEntry,
    TodoStatus, TodosRequest, TurnRequest, TurnSummary, UpdateThreadRequest, UsageResponse,
    WorkspaceChange, WorkspaceChangesResponse, WorkspaceDescriptor, WorkspaceDiff,
    WorkspaceDiffFile, WorkspaceView, LLM_ROUNDS_DEFAULT_LIMIT, LLM_ROUNDS_MAX_LIMIT,
    LLM_ROUNDS_MAX_THREADS, MAX_TURN_TEXT_BYTES, REASONING_EFFORTS, THREAD_EXPORT_FORMAT_VERSION,
    THREAD_PAGE_DEFAULT_LIMIT, THREAD_PAGE_MAX_LIMIT,
};
pub use event::{
    ChildCompletionNotice, EventKind, RuntimeEvent, TurnAttachmentDelivery, TurnAttachmentOutcome,
};
pub use im::{
    CreateImChannelRequest, FeishuDomain, ImBinding, ImBindingListResponse, ImChannel, ImChannelId,
    ImChannelKind, ImChannelListItem, ImChannelListResponse, ImChannelPolicy, ImChannelRuntime,
    ImChannelState, ImChannelView, ImCredentials, ImCredentialsView, ImVerifyResponse,
    ImWeixinLoginStartResponse, ImWeixinLoginStatus, ImWeixinLoginWaitResponse,
    UpdateImChannelRequest, DEFAULT_IM_AUTONOMY, IM_TITLE_MAX_BYTES, MAX_IM_CHANNELS,
};
pub use item::{ItemKind, ItemMarker, ItemStatus, ThreadItem};
pub use schedule::{
    CreateScheduleRequest, RunStatus, ScheduleKind, ScheduledRunRecord, ScheduledTask,
    ScheduledTaskListResponse, TaskSchedule, UpdateScheduleRequest, DEFAULT_SCHEDULE_AUTONOMY,
    DEFAULT_TIME_OF_DAY, MAX_EVERY_MINUTES, MAX_SCHEDULED_TASKS, MIN_EVERY_MINUTES,
    SCHEDULE_RUN_HISTORY_LIMIT, SCHEDULE_TITLE_MAX_BYTES,
};
