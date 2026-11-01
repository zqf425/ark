//! Pure plugin rules: manifest, identity, topology, path containment.

pub mod directory;
pub mod identity;
pub mod isolation;
pub mod manifest;
pub mod path;
pub mod topology;

pub use directory::{DirectoryManifest, PageDecl, PanelDecl, ToolAction, ToolDecl, UiSection};
pub(crate) use identity::grant_for;
pub use identity::{validate_plugin_id, Grant, PluginIdentity};
pub use isolation::{plan_local_load, IsolationPlan, PluginDrop};
pub use manifest::{HttpSection, McpSection, PluginKind, PluginManifest, Version, VersionReq};
pub use path::{resolve_within_root, sanitize_relative};
pub use topology::{plan_skips, resolve_order, PluginId, TopologyErrorKind};
