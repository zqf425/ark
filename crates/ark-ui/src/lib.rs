//! ark-ui: Base capability: UI bus, page/panel registry (design 7.5.3).
//!
//! Layer folders follow the QDC mapping in design 5.1:
//!
//! * [`domain`] - the page/panel catalog and capability gate. Pure maps, no
//!   locks, no I/O, so conflict rules are unit-testable.
//! * [`service`] - [`UiBus`] (the [`ark_ports::UiCapability`] implementation)
//!   plus the `ark.builtin.code-ui` plugin definition.
//!
//! ## One catalog, three consumers
//!
//! [`UiBus::catalog_json`] is what `GET /v1/runtime/capabilities` serves, what
//! the renderer builds its navigation table from, and what the shell router
//! resolves. There is no second list of pages anywhere in the runtime.
//!
//! ## Registration is scoped
//!
//! [`UiBus::register_page`] returns [`PageRegistration`]; dropping it removes the
//! page and frees its route. A plugin unload therefore cannot leave a dead
//! route behind, and a duplicate id or route is a hard conflict instead of a
//! silent override.

#![deny(clippy::all)]

pub mod domain;
pub mod service;

pub use domain::{require_caps, PageIndex, PanelIndex, UiCatalog, SYSTEM_OWNER};
pub use service::{
    code_page, code_panels, PageRegistration, PanelRegistration, UiBus, CHANGES_PANEL_ID,
    CODE_BUNDLE, CODE_PAGE_ID, CODE_ROUTE, CODE_UI_PLUGIN_ID, FILE_PANEL_ID, PLAN_PANEL_ID,
    TERMINAL_PANEL_ID,
};
