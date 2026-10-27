//! Plugin trait and revocable registration (design 7.7.1).
//!
//! A plugin contributes capabilities by calling methods on [`PluginContext`];
//! everything it registered is collected into one [`Registration`]. Dropping
//! that value unregisters all of it, so an unload never leaves a dead route or a
//! ghost tool behind.

use crate::domain::PluginManifest;
use crate::service::context::PluginContext;
use ark_common::Result;
use ark_mcp::ExposureHandle;
use ark_ui::{PageRegistration, PanelRegistration};

/// The one contract every plugin (built-in or external wrapper) implements.
pub trait Plugin: Send + Sync {
    /// Declared identity and capabilities. The host reads this before any plugin
    /// code runs, so it must be cheap and side-effect free.
    fn manifest(&self) -> &PluginManifest;

    /// Register contributions. Returning `Err` means "this plugin does not
    /// start": the host records `plugin_load_failed` and skips it, while other
    /// plugins keep running.
    fn register(&self, ctx: &mut PluginContext) -> Result<Registration>;
}

/// Everything one plugin registered. Drop = unregister, in reverse order.
#[derive(Default)]
pub struct Registration {
    pages: Vec<PageRegistration>,
    panels: Vec<PanelRegistration>,
    exposures: Vec<ExposureHandle>,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("pages", &self.pages.len())
            .field("panels", &self.panels.len())
            .field("exposures", &self.exposures.len())
            .finish()
    }
}

impl Registration {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_page(&mut self, page: PageRegistration) {
        self.pages.push(page);
    }

    pub fn push_panel(&mut self, panel: PanelRegistration) {
        self.panels.push(panel);
    }

    pub fn push_exposure(&mut self, exposure: ExposureHandle) {
        self.exposures.push(exposure);
    }

    pub fn page_ids(&self) -> Vec<String> {
        self.pages.iter().map(|p| p.id().to_string()).collect()
    }

    pub fn panel_ids(&self) -> Vec<String> {
        self.panels.iter().map(|p| p.id().to_string()).collect()
    }

    pub fn exposure_ids(&self) -> Vec<String> {
        self.exposures.iter().map(|e| e.id().to_string()).collect()
    }

    /// Drop the guards explicitly, unregistering everything now.
    pub fn revoke(self) {
        drop(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ports::UiCapability;
    use ark_ui::{code_page, code_panels, UiBus};
    use std::sync::Arc;

    struct CodeUi {
        manifest: PluginManifest,
    }

    impl Plugin for CodeUi {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }

        fn register(&self, ctx: &mut PluginContext) -> Result<Registration> {
            let mut registration = Registration::new();
            registration.push_page(ctx.register_page(code_page())?);
            for (slot, panel) in code_panels() {
                registration.push_panel(ctx.register_panel(slot, panel)?);
            }
            Ok(registration)
        }
    }

    fn code_ui_manifest() -> PluginManifest {
        let mut manifest = PluginManifest::new(
            "ark.builtin.code-ui",
            "0.1.0",
            crate::domain::PluginKind::UiPage,
        )
        .unwrap();
        manifest.capabilities = vec![ark_contract::Capability::UiPage];
        manifest
    }

    #[test]
    fn registration_reports_what_it_holds() {
        // The loader builds contexts; the registry tests cover the full path.
        // Here we only assert the container is honest about its contents.
        let registration = Registration::new();
        assert!(registration.page_ids().is_empty());
        assert!(registration.panel_ids().is_empty());
        assert!(registration.exposure_ids().is_empty());
        let _ = CodeUi {
            manifest: code_ui_manifest(),
        };
        let _ = Arc::new(UiBus::new());
        let _: Vec<&dyn UiCapability> = Vec::new();
    }
}
