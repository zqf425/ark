//! Page and panel catalog: the single source of navigation (design 7.5.3).
//!
//! Both `GET /v1/runtime/capabilities` and the renderer navigation table read
//! [`UiCatalog`]. There is deliberately no second list, and every read is sorted
//! so the catalog is byte-stable between polls.

use ark_common::{ArkError, Result};
use ark_contract::{Capability, PageSpec, PanelEntry, PanelSlot, PanelSpec, UiCatalog};
use std::collections::{BTreeMap, HashSet};

/// Panel slot ordering for rendering: left, right, bottom.
fn slot_rank(slot: PanelSlot) -> u8 {
    match slot {
        PanelSlot::Left => 0,
        PanelSlot::Right => 1,
        PanelSlot::Bottom => 2,
    }
}

/// Page table plus the route index that enforces "one route, one page".
#[derive(Debug, Default)]
pub struct PageIndex {
    pages: BTreeMap<String, PageSpec>,
    routes: BTreeMap<String, String>,
}

impl PageIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or fail. Duplicate ids and duplicate routes are *conflicts*, not
    /// silent overrides: two plugins claiming `/code` is a manifest bug.
    pub fn insert(&mut self, page: PageSpec) -> Result<()> {
        let id = page.id.0.clone();
        if id.trim().is_empty() {
            return Err(ArkError::validation("page id is empty").with_detail("invalid_page_id"));
        }
        if let Some(existing) = self.pages.get(&id) {
            return Err(ArkError::conflict(format!(
                "page id {id} is already registered on route {}",
                existing.route
            ))
            .with_detail("duplicate_page_id"));
        }
        if !page.route.starts_with('/') {
            return Err(ArkError::validation(format!(
                "page route {:?} must start with /",
                page.route
            ))
            .with_detail("invalid_route"));
        }
        if let Some(owner) = self.routes.get(&page.route) {
            return Err(ArkError::conflict(format!(
                "route {} is already owned by page {owner}",
                page.route
            ))
            .with_detail("duplicate_route"));
        }
        self.routes.insert(page.route.clone(), id.clone());
        self.pages.insert(id, page);
        Ok(())
    }

    /// Remove by id; returns the removed page. Used by `Drop` handles.
    pub fn remove(&mut self, id: &str) -> Option<PageSpec> {
        let page = self.pages.remove(id)?;
        self.routes.remove(&page.route);
        Some(page)
    }

    pub fn get(&self, id: &str) -> Option<&PageSpec> {
        self.pages.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.pages.contains_key(id)
    }

    pub fn route_owner(&self, route: &str) -> Option<&String> {
        self.routes.get(route)
    }

    /// Sorted by id: stable order for the nav table and the HTTP payload.
    pub fn snapshot(&self) -> Vec<PageSpec> {
        self.pages.values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }
}

/// Panels keyed by id across every slot: a plugin cannot register the same
/// panel id twice by moving it to another slot.
#[derive(Debug, Default)]
pub struct PanelIndex {
    panels: BTreeMap<String, (PanelSlot, PanelSpec)>,
}

impl PanelIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, slot: PanelSlot, panel: PanelSpec) -> Result<()> {
        let id = panel.id.0.clone();
        if id.trim().is_empty() {
            return Err(ArkError::validation("panel id is empty").with_detail("invalid_panel_id"));
        }
        if panel.slot != slot {
            return Err(ArkError::validation(format!(
                "panel {id} declares slot {:?} but was registered into {:?}",
                panel.slot, slot
            ))
            .with_detail("panel_slot_mismatch"));
        }
        if self.panels.contains_key(&id) {
            return Err(
                ArkError::conflict(format!("panel id {id} is already registered"))
                    .with_detail("duplicate_panel_id"),
            );
        }
        self.panels.insert(id, (slot, panel));
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Option<(PanelSlot, PanelSpec)> {
        self.panels.remove(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.panels.contains_key(id)
    }

    /// Slot-major, then id: matches render order and stays stable.
    pub fn snapshot(&self) -> Vec<(PanelSlot, PanelSpec)> {
        let mut out: Vec<(PanelSlot, PanelSpec)> = self
            .panels
            .values()
            .map(|(slot, spec)| (*slot, spec.clone()))
            .collect();
        out.sort_by(|a, b| {
            slot_rank(a.0)
                .cmp(&slot_rank(b.0))
                .then_with(|| a.1.id.0.cmp(&b.1.id.0))
        });
        out
    }

    pub fn len(&self) -> usize {
        self.panels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }
}

/// Builds the wire catalog from the sorted page/panel snapshots.
///
/// The shape lives in `ark-contract` (`UiCatalog`/`PanelEntry`), so the HTTP
/// response and the renderer's generated types cannot drift from each other.
pub fn to_contract_catalog(pages: Vec<PageSpec>, panels: Vec<(PanelSlot, PanelSpec)>) -> UiCatalog {
    UiCatalog {
        pages,
        panels: panels
            .into_iter()
            .map(|(slot, panel)| PanelEntry { slot, panel })
            .collect(),
    }
}

/// Capability gate: a page or panel whose `required_caps` are not satisfied is
/// hidden, not shown broken.
pub fn require_caps(required: &[Capability], available: &HashSet<Capability>) -> Result<()> {
    let missing: Vec<String> = required
        .iter()
        .filter(|cap| !available.contains(cap))
        .map(|cap| format!("{cap:?}"))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(
        ArkError::validation(format!("missing capabilities: {}", missing.join(", ")))
            .with_detail("missing_capabilities"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::{PageId, Placement};

    fn page(id: &str, route: &str) -> PageSpec {
        PageSpec {
            id: PageId::new(id),
            route: route.to_string(),
            title: "Title".into(),
            icon: None,
            placement: Placement::PrimaryNav,
            bundle: None,
            required_caps: vec![],
        }
    }

    fn panel(id: &str, slot: PanelSlot) -> PanelSpec {
        PanelSpec {
            id: PageId::new(id),
            slot,
            title: "Panel".into(),
            required_caps: vec![],
        }
    }

    #[test]
    fn duplicate_page_id_is_a_conflict() {
        let mut index = PageIndex::new();
        index.insert(page("ark.a", "/a")).unwrap();
        let err = index.insert(page("ark.a", "/other")).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("duplicate_page_id"));
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn duplicate_route_is_a_conflict() {
        let mut index = PageIndex::new();
        index.insert(page("ark.a", "/code")).unwrap();
        let err = index.insert(page("ark.b", "/code")).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("duplicate_route"));
        assert_eq!(index.len(), 1, "the losing page must not be registered");
    }

    #[test]
    fn route_must_be_absolute() {
        let mut index = PageIndex::new();
        let err = index.insert(page("ark.a", "a")).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("invalid_route"));
    }

    #[test]
    fn remove_frees_both_the_id_and_the_route() {
        let mut index = PageIndex::new();
        index.insert(page("ark.a", "/code")).unwrap();
        assert!(index.remove("ark.a").is_some());
        assert!(!index.contains("ark.a"));
        index
            .insert(page("ark.b", "/code"))
            .expect("route must be reusable");
        assert_eq!(
            index.route_owner("/code").map(String::as_str),
            Some("ark.b")
        );
    }

    #[test]
    fn snapshot_is_sorted_by_id() {
        let mut index = PageIndex::new();
        for (id, route) in [("ark.c", "/c"), ("ark.a", "/a"), ("ark.b", "/b")] {
            index.insert(page(id, route)).unwrap();
        }
        let ids: Vec<String> = index.snapshot().into_iter().map(|p| p.id.0).collect();
        assert_eq!(ids, vec!["ark.a", "ark.b", "ark.c"]);
    }

    #[test]
    fn duplicate_panel_id_is_a_conflict_across_slots() {
        let mut index = PanelIndex::new();
        index
            .insert(PanelSlot::Right, panel("ark.p", PanelSlot::Right))
            .unwrap();
        let err = index
            .insert(PanelSlot::Bottom, panel("ark.p", PanelSlot::Bottom))
            .unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("duplicate_panel_id"));
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn panel_slot_must_match_the_registration_slot() {
        let mut index = PanelIndex::new();
        let err = index
            .insert(PanelSlot::Left, panel("ark.p", PanelSlot::Right))
            .unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("panel_slot_mismatch"));
    }

    #[test]
    fn panels_render_in_slot_order() {
        let mut index = PanelIndex::new();
        for (id, slot) in [
            ("ark.bottom", PanelSlot::Bottom),
            ("ark.right", PanelSlot::Right),
            ("ark.left", PanelSlot::Left),
        ] {
            index.insert(slot, panel(id, slot)).unwrap();
        }
        let slots: Vec<PanelSlot> = index.snapshot().into_iter().map(|(slot, _)| slot).collect();
        assert_eq!(
            slots,
            vec![PanelSlot::Left, PanelSlot::Right, PanelSlot::Bottom]
        );
    }

    #[test]
    fn require_caps_lists_missing_capabilities() {
        let available: HashSet<Capability> = [Capability::Tool].into_iter().collect();
        assert!(require_caps(&[Capability::Tool], &available).is_ok());
        let err = require_caps(&[Capability::Tool, Capability::Backend], &available).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("missing_capabilities"));
        assert!(err.message.contains("Backend"));
    }

    #[test]
    fn contract_catalog_carries_pages_and_panels() {
        let catalog = to_contract_catalog(
            vec![page("ark.a", "/a")],
            vec![(PanelSlot::Right, panel("ark.p", PanelSlot::Right))],
        );
        assert_eq!(catalog.pages.len(), 1);
        assert_eq!(catalog.panels.len(), 1);
        assert_eq!(catalog.panels[0].slot, PanelSlot::Right);
        assert_eq!(catalog.panels[0].panel.id.0, "ark.p");
    }
}
