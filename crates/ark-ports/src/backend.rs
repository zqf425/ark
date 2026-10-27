//! Backend integration port (design §7.12).
//!
//! Only domain concepts appear here: no URL prefix, no platform token shape, no
//! vendor field. `ark-studio` depends on this trait alone, and switching backend
//! is a plugin registration rather than a core change.

use ark_common::{Result, Usage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CaseId(pub String);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthContext {
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub token_ref: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendSession {
    pub authenticated: bool,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaseQuery {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaseSummary {
    pub id: CaseId,
    pub title: String,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaseDetail {
    pub summary: CaseSummary,
    #[serde(default)]
    pub steps: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaseDraft {
    pub id: CaseId,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaseVersionRef {
    pub case_id: CaseId,
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublishRequest {
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublishResult {
    pub published: bool,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunPlan {
    #[serde(default)]
    pub headed: bool,
    #[serde(default)]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionRef {
    pub case_id: CaseId,
    pub execution_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionState {
    pub status: String,
    #[serde(default)]
    pub finished: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub run_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteArtifactRef {
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunOutcome {
    pub passed: bool,
    #[serde(default)]
    pub steps: u64,
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// The one contract a backend platform has to satisfy.
#[async_trait]
pub trait BackendProvider: Send + Sync {
    fn id(&self) -> &'static str;

    fn display_name(&self) -> &str;

    async fn authenticate(&self, ctx: &AuthContext) -> Result<BackendSession>;

    async fn list_cases(&self, q: CaseQuery) -> Result<Page<CaseSummary>>;

    async fn get_case(&self, id: &CaseId) -> Result<CaseDetail>;

    async fn save_draft(&self, case: &CaseDraft) -> Result<CaseVersionRef>;

    async fn publish(&self, v: &CaseVersionRef, req: PublishRequest) -> Result<PublishResult>;

    async fn trigger_execution(&self, case: &CaseId, plan: &RunPlan) -> Result<ExecutionRef>;

    async fn poll_execution(&self, e: &ExecutionRef) -> Result<ExecutionState>;

    async fn upload_artifact(&self, a: &ArtifactRef) -> Result<RemoteArtifactRef>;

    async fn report_result(&self, e: &ExecutionRef, r: &RunOutcome) -> Result<()>;
}
