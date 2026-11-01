//! ark-ports: every boundary the core depends on, expressed as a trait.
//!
//! Design §5.2 makes this crate the *only* dependency of `ark-agent`, so the
//! agent loop never learns about `reqwest`, SQLite, axum or a concrete provider.
//! Implementations live in `arkd`'s wiring module; nothing here performs I/O.

pub mod attachment;
pub mod backend;
pub mod base;
pub mod bus;
pub mod event;
pub mod model;
pub mod services;
pub mod store;
pub mod subagent;
pub mod tool;

pub use attachment::{AttachmentStore, AttachmentUpload};
pub use backend::{
    ArtifactRef, AuthContext, BackendProvider, BackendSession, CaseDetail, CaseDraft, CaseId,
    CaseQuery, CaseSummary, CaseVersionRef, ExecutionRef, ExecutionState, Page, PublishRequest,
    PublishResult, RemoteArtifactRef, RunOutcome, RunPlan,
};
pub use base::{
    BaseServices, McpServerService, MemoryService, ModelEndpoint, PluginHostService,
    ProviderService, SkillService, WorkspaceService,
};
pub use bus::{
    HeaderMap, HttpCapability, McpCallResult, McpCapability, McpCatalog, McpToolSpec,
    OutboundRequest, OutboundResponse, PageHandle, PanelHandle, UiCapability,
};
pub use event::{EventSink, Subscription};
pub use model::{
    ModelChunk, ModelClient, ModelMessage, ModelRequest, ModelResponse, ModelRole, ToolCallRef,
};
pub use services::{
    ConfigView, CredentialService, FileService, KvService, LogService, MetricService,
    NotifyService, TraceService,
};
pub use store::{latest_child_runs, ChildRunRecord, SessionStore, SubscribeTicket, ThreadMeta};
pub use subagent::{
    status as child_status, stop_reason, CancelHandle, SteerHandle, SubagentOutcome,
    SubagentPolicy, SubagentPolicySource, SubagentRegistry, SubagentRequest, SubagentRunner,
};
pub use tool::{
    SandboxDecision, SandboxPolicy, Tool, ToolArgs, ToolContext, ToolOutput, ToolRegistryPort,
};
