//! Capability catalogue: one source of truth for `GET /v1/runtime/capabilities`,
//! MCP `tools/list` and the renderer navigation table (design §7.5.2, §7.7.1).

use serde::{Deserialize, Serialize};

/// A capability a plugin contributes or a page requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Tool,
    ModelProvider,
    McpClient,
    Studio,
    Publish,
    ArtifactUpload,
    UiPage,
    UiPanel,
    Backend,
    Im,
}

/// Stable page identifier, e.g. `ark.builtin.code`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(transparent)]
pub struct PageId(pub String);

impl PageId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl From<&str> for PageId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Where a page shows up in the shell navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    PrimaryNav,
    SecondaryNav,
    Settings,
    Hidden,
}

/// Panel slots a plugin may register into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum PanelSlot {
    Right,
    Bottom,
    Left,
}

/// A page contributed to the UI bus. `plugin-code-ui` registers exactly one of
/// these, so the Code screen has no privileged path in the shell.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct PageSpec {
    pub id: PageId,
    pub route: String,
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub placement: Placement,
    #[serde(default)]
    pub bundle: Option<String>,
    #[serde(default)]
    pub required_caps: Vec<Capability>,
}

/// A panel contributed to an existing page slot.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct PanelSpec {
    pub id: PageId,
    pub slot: PanelSlot,
    pub title: String,
    #[serde(default)]
    pub required_caps: Vec<Capability>,
}
