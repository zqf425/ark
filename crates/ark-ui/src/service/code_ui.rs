//! `plugin-code-ui`: the Code screen as a regular UI plugin (design 7.5.3).
//!
//! The Code screen has no privileged path in the shell: it registers one page
//! and four panels through the same API any third-party plugin uses. Disabling
//! this plugin must leave the application startable with only the navigation
//! skeleton, which is why nothing here touches routing or the window.
//!
//! The *data* (titles, slots, bundle refs) is defined here as constants so
//! `arkd` can inject it without the shell hard-coding any of these ids.

use ark_contract::{Capability, PageId, PageSpec, PanelSlot, PanelSpec, Placement};
use ark_ports::UiCapability;

/// Stable plugin id of this built-in plugin.
pub const PLUGIN_ID: &str = "ark.builtin.code-ui";
/// Page id of the Code screen.
pub const CODE_PAGE_ID: &str = "ark.builtin.code";
/// Route the shell resolves to the Code screen.
pub const CODE_ROUTE: &str = "/code";
/// Front-end bundle key; the built-in bundle is compiled into the app.
pub const CODE_BUNDLE: &str = "builtin:code";

pub const CHANGES_PANEL_ID: &str = "ark.builtin.code.changes";
pub const PLAN_PANEL_ID: &str = "ark.builtin.code.plan";
pub const FILE_PANEL_ID: &str = "ark.builtin.code.file";
pub const TERMINAL_PANEL_ID: &str = "ark.builtin.code.terminal";

/// The Code page: primary navigation, tool + model capability gated.
pub fn code_page() -> PageSpec {
    PageSpec {
        id: PageId::new(CODE_PAGE_ID),
        route: CODE_ROUTE.to_string(),
        title: "Code".to_string(),
        icon: Some("code".to_string()),
        placement: Placement::PrimaryNav,
        bundle: Some(CODE_BUNDLE.to_string()),
        required_caps: vec![Capability::Tool, Capability::ModelProvider],
    }
}

/// The four panels the Code screen embeds.
///
/// * `Changes` and `File` live on the right rail.
/// * `Plan` and `Terminal` share the bottom dock.
pub fn code_panels() -> Vec<(PanelSlot, PanelSpec)> {
    vec![
        (
            PanelSlot::Right,
            PanelSpec {
                id: PageId::new(CHANGES_PANEL_ID),
                slot: PanelSlot::Right,
                title: "Changes".to_string(),
                required_caps: vec![Capability::Tool],
            },
        ),
        (
            PanelSlot::Right,
            PanelSpec {
                id: PageId::new(FILE_PANEL_ID),
                slot: PanelSlot::Right,
                title: "File".to_string(),
                required_caps: vec![],
            },
        ),
        (
            PanelSlot::Bottom,
            PanelSpec {
                id: PageId::new(PLAN_PANEL_ID),
                slot: PanelSlot::Bottom,
                title: "Plan".to_string(),
                required_caps: vec![],
            },
        ),
        (
            PanelSlot::Bottom,
            PanelSpec {
                id: PageId::new(TERMINAL_PANEL_ID),
                slot: PanelSlot::Bottom,
                title: "Terminal".to_string(),
                required_caps: vec![Capability::Tool],
            },
        ),
    ]
}

/// Convenience for `arkd`: register page + panels through the port surface.
///
/// Returns the page handle id so the caller can log what it registered. Panels
/// are registered through the port, which keeps them for the process lifetime.
pub fn register(ui: &dyn UiCapability) -> ark_common::Result<String> {
    let page = ui.register_page(code_page())?;
    for (slot, panel) in code_panels() {
        ui.register_panel(slot, panel)?;
    }
    Ok(page.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_page_is_primary_nav_with_bundle() {
        let page = code_page();
        assert_eq!(page.id.0, CODE_PAGE_ID);
        assert_eq!(page.route, CODE_ROUTE);
        assert_eq!(page.placement, Placement::PrimaryNav);
        assert_eq!(page.bundle.as_deref(), Some(CODE_BUNDLE));
        assert!(page.route.starts_with('/'));
    }

    #[test]
    fn four_panels_cover_changes_plan_file_terminal() {
        let panels = code_panels();
        assert_eq!(panels.len(), 4);
        let ids: Vec<&str> = panels.iter().map(|(_, p)| p.id.0.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                CHANGES_PANEL_ID,
                FILE_PANEL_ID,
                PLAN_PANEL_ID,
                TERMINAL_PANEL_ID
            ]
        );
    }

    #[test]
    fn panel_slots_match_their_spec() {
        for (slot, panel) in code_panels() {
            assert_eq!(slot, panel.slot, "panel {} slot mismatch", panel.id.0);
        }
    }

    #[test]
    fn right_rail_and_bottom_dock_are_both_used() {
        let slots: Vec<PanelSlot> = code_panels().into_iter().map(|(slot, _)| slot).collect();
        assert_eq!(slots.iter().filter(|s| **s == PanelSlot::Right).count(), 2);
        assert_eq!(slots.iter().filter(|s| **s == PanelSlot::Bottom).count(), 2);
    }

    #[test]
    fn ids_are_namespaced_under_the_plugin() {
        assert!(code_page().id.0.starts_with("ark.builtin."));
        for (_, panel) in code_panels() {
            assert!(panel.id.0.starts_with("ark.builtin.code."));
        }
    }
}
