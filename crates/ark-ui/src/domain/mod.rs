//! Pure UI-bus rules: the page/panel catalog and capability gating.

pub mod catalog;

pub use catalog::{require_caps, to_contract_catalog, PageIndex, PanelIndex};

/// Owner id used for host-registered pages that are not tied to a plugin
/// lifetime. It is deliberately not a valid plugin id (leading `_`).
pub const SYSTEM_OWNER: &str = "_host";
