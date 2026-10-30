//! The UI capability implementation (design 7.5.3).
//!
//! One catalog, three consumers: `GET /v1/runtime/capabilities`, the renderer
//! navigation table and the shell router. Registration is scoped: the guard
//! returned by [`UiBus::register_page`] unregisters on `Drop`, so unloading a
//! plugin cannot leave a dead route behind.

use crate::domain::catalog::{require_caps, PageIndex, PanelIndex, UiCatalog};
use ark_common::{ArkError, Result};
use ark_contract::{Capability, PageSpec, PanelSlot, PanelSpec};
use ark_ports::{PageHandle, PanelHandle, UiCapability};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

type Shared = Arc<Mutex<Pages>>;

/// Mutable state behind one lock so a page and its route move atomically.
#[derive(Debug, Default)]
struct Pages {
    pages: PageIndex,
    panels: PanelIndex,
    /// Page id -> owning plugin id, for scoped deregistration.
    page_owners: BTreeMap<String, String>,
    panel_owners: BTreeMap<String, String>,
}

/// The shell-facing UI bus.
#[derive(Debug)]
pub struct UiBus {
    state: Shared,
    available: Mutex<HashSet<Capability>>,
}

impl UiBus {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(Pages::default())),
            available: Mutex::new(HashSet::new()),
        }
    }

    /// Host declares which capabilities currently exist. A page whose
    /// `required_caps` are not all present is hidden, never shown broken.
    pub fn grant_capability(&self, capability: Capability) {
        if let Ok(mut available) = self.available.lock() {
            available.insert(capability);
        }
    }

    pub fn revoke_capability(&self, capability: Capability) {
        if let Ok(mut available) = self.available.lock() {
            available.remove(&capability);
        }
    }

    pub fn available_capabilities(&self) -> HashSet<Capability> {
        self.available
            .lock()
            .map(|set| set.clone())
            .unwrap_or_default()
    }

    /// Capability gate used by the shell before routing to a page.
    pub fn require(&self, caps: &[Capability]) -> Result<()> {
        require_caps(caps, &self.available_capabilities())
    }

    /// Register a page owned by `plugin_id`; the returned guard deregisters on
    /// `Drop`.
    pub fn register_page(&self, plugin_id: &str, page: PageSpec) -> Result<PageRegistration> {
        let id = page.id.0.clone();
        let mut state = self.lock()?;
        state.pages.insert(page)?;
        state.page_owners.insert(id.clone(), plugin_id.to_string());
        drop(state);
        Ok(PageRegistration {
            id,
            owner: plugin_id.to_string(),
            bus: Arc::downgrade(&self.state),
        })
    }

    /// Register a panel owned by `plugin_id`.
    pub fn register_panel(
        &self,
        plugin_id: &str,
        slot: PanelSlot,
        panel: PanelSpec,
    ) -> Result<PanelRegistration> {
        let id = panel.id.0.clone();
        let mut state = self.lock()?;
        state.panels.insert(slot, panel)?;
        state.panel_owners.insert(id.clone(), plugin_id.to_string());
        drop(state);
        Ok(PanelRegistration {
            id,
            owner: plugin_id.to_string(),
            bus: Arc::downgrade(&self.state),
        })
    }

    /// Panels in render order (slot-major, then id).
    pub fn panels(&self) -> Vec<(PanelSlot, PanelSpec)> {
        self.state
            .lock()
            .map(|state| state.panels.snapshot())
            .unwrap_or_default()
    }

    /// The whole catalog: pages plus panels, sorted and stable.
    pub fn catalog(&self) -> UiCatalog {
        UiCatalog {
            pages: self.pages(),
            panels: self.panels(),
        }
    }

    /// Serialized catalog for `GET /v1/runtime/capabilities`.
    pub fn catalog_json(&self) -> serde_json::Value {
        self.catalog().to_json()
    }

    /// Drop every page and panel contributed by one plugin (unload path).
    pub fn unregister_plugin(&self, plugin_id: &str) -> usize {
        let Ok(mut state) = self.lock() else {
            return 0;
        };
        let page_ids: Vec<String> = state
            .page_owners
            .iter()
            .filter(|(_, owner)| owner.as_str() == plugin_id)
            .map(|(id, _)| id.clone())
            .collect();
        let panel_ids: Vec<String> = state
            .panel_owners
            .iter()
            .filter(|(_, owner)| owner.as_str() == plugin_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &page_ids {
            state.pages.remove(id);
            state.page_owners.remove(id);
        }
        for id in &panel_ids {
            state.panels.remove(id);
            state.panel_owners.remove(id);
        }
        page_ids.len() + panel_ids.len()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Pages>> {
        self.state
            .lock()
            .map_err(|_| ArkError::internal("ui registry poisoned"))
    }
}

impl Default for UiBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Scoped page registration. Dropping it removes the page and frees its route.
#[derive(Debug)]
pub struct PageRegistration {
    id: String,
    owner: String,
    bus: Weak<Mutex<Pages>>,
}

impl PageRegistration {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Convert to the port-level handle without dropping the registration,
    /// which is what a long-lived built-in page needs.
    pub fn handle(&self) -> PageHandle {
        PageHandle {
            id: self.id.clone(),
        }
    }

    /// Give up the guard: the page stays registered for the process lifetime.
    pub fn forget(self) {
        std::mem::forget(self);
    }
}

impl Drop for PageRegistration {
    fn drop(&mut self) {
        let Some(bus) = self.bus.upgrade() else {
            return;
        };
        let Ok(mut state) = bus.lock() else {
            return;
        };
        // Only the owner may remove: a stale guard must not delete a page that
        // another plugin has since registered under the same id.
        let owned = state
            .page_owners
            .get(&self.id)
            .map(|owner| owner == &self.owner)
            .unwrap_or(false);
        if owned {
            state.pages.remove(&self.id);
            state.page_owners.remove(&self.id);
        }
    }
}

/// Scoped panel registration.
#[derive(Debug)]
pub struct PanelRegistration {
    id: String,
    owner: String,
    bus: Weak<Mutex<Pages>>,
}

impl PanelRegistration {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn handle(&self) -> PanelHandle {
        PanelHandle {
            id: self.id.clone(),
        }
    }

    pub fn forget(self) {
        std::mem::forget(self);
    }
}

impl Drop for PanelRegistration {
    fn drop(&mut self) {
        let Some(bus) = self.bus.upgrade() else {
            return;
        };
        let Ok(mut state) = bus.lock() else {
            return;
        };
        let owned = state
            .panel_owners
            .get(&self.id)
            .map(|owner| owner == &self.owner)
            .unwrap_or(false);
        if owned {
            state.panels.remove(&self.id);
            state.panel_owners.remove(&self.id);
        }
    }
}

#[async_trait]
impl UiCapability for UiBus {
    /// Port-level registration without an owner. The page stays for the
    /// process lifetime; use [`UiBus::register_page`] when the caller needs
    /// `Drop` deregistration or ownership tracking.
    fn register_page(&self, page: PageSpec) -> Result<PageHandle> {
        let id = page.id.0.clone();
        self.register_page(crate::domain::SYSTEM_OWNER, page)?
            .forget();
        Ok(PageHandle { id })
    }

    fn register_panel(&self, slot: PanelSlot, panel: PanelSpec) -> Result<PanelHandle> {
        let id = panel.id.0.clone();
        self.register_panel(crate::domain::SYSTEM_OWNER, slot, panel)?
            .forget();
        Ok(PanelHandle { id })
    }

    fn pages(&self) -> Vec<PageSpec> {
        self.state
            .lock()
            .map(|state| state.pages.snapshot())
            .unwrap_or_default()
    }

    fn capabilities(&self) -> Vec<Capability> {
        // Sorted so the response is byte-stable across restarts; iterating the
        // `HashSet` directly would make the same runtime answer differently.
        let mut caps: Vec<Capability> = self.available_capabilities().into_iter().collect();
        caps.sort_by_key(|cap| format!("{cap:?}"));
        caps
    }

    fn catalog_json(&self) -> serde_json::Value {
        UiBus::catalog_json(self)
    }
}
