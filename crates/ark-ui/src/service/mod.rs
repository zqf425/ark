//! Service layer of the UI bus.

pub mod bus;
pub mod code_ui;

pub use bus::{PageRegistration, PanelRegistration, UiBus};
pub use code_ui::{
    code_page, code_panels, register, CHANGES_PANEL_ID, CODE_BUNDLE, CODE_PAGE_ID, CODE_ROUTE,
    FILE_PANEL_ID, PLAN_PANEL_ID, PLUGIN_ID as CODE_UI_PLUGIN_ID, TERMINAL_PANEL_ID,
};
