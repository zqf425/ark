//! Startup dependency resolution (design 7.5.4, 7.7.5).
//!
//! Everything here is pure: manifests in, plugin order out. Cycle, missing
//! service and major-version incompatibility all fail **before** any plugin code
//! runs, so there is no runtime fallback to hide a broken declaration.

use ark_common::{ArkError, Result};
use std::collections::{BTreeMap, BTreeSet};

use crate::domain::manifest::{PluginManifest, VersionReq};

/// Stable plugin identifier.
pub type PluginId = String;

/// Why a manifest set cannot be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyErrorKind {
    DuplicateId,
    MissingDependency,
    MajorMismatch,
    Cycle,
}

impl TopologyErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TopologyErrorKind::DuplicateId => "plugin_duplicate_id",
            TopologyErrorKind::MissingDependency => "plugin_missing_dependency",
            TopologyErrorKind::MajorMismatch => "plugin_version_incompatible",
            TopologyErrorKind::Cycle => "plugin_dependency_cycle",
        }
    }
}

/// Resolve a start order: every plugin appears after the services it requires.
///
/// The order is deterministic (ties broken by plugin id) so a lock manifest and
/// a startup log are comparable between runs.
pub fn resolve_order(manifests: &[PluginManifest]) -> Result<Vec<PluginId>> {
    let index = index_manifests(manifests)?;
    let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();

    for manifest in manifests {
        let mut required: BTreeSet<&str> = BTreeSet::new();
        for (service, requirement) in &manifest.requires {
            let target = index.get(service.as_str()).ok_or_else(|| {
                fail(
                    TopologyErrorKind::MissingDependency,
                    &manifest.id,
                    format!("requires {service} {requirement} but no such plugin is installed"),
                )
            })?;
            let req = VersionReq::parse(requirement)?;
            if !req.accepts(&target.version) {
                return Err(fail(
                    TopologyErrorKind::MajorMismatch,
                    &manifest.id,
                    format!(
                        "requires {service} {requirement} but installed version is {}",
                        target.version
                    ),
                ));
            }
            required.insert(service.as_str());
        }
        deps.insert(manifest.id.as_str(), required);
    }

    // Kahn's algorithm over a sorted ready set: deterministic, and a leftover
    // node is exactly the cycle case.
    let mut remaining: BTreeSet<&str> = manifests.iter().map(|m| m.id.as_str()).collect();
    let mut order: Vec<PluginId> = Vec::with_capacity(manifests.len());
    while !remaining.is_empty() {
        let ready: Vec<&str> = remaining
            .iter()
            .copied()
            .filter(|id| {
                deps.get(id)
                    .map(|set| set.iter().all(|dep| !remaining.contains(dep)))
                    .unwrap_or(true)
            })
            .collect();
        if ready.is_empty() {
            let cycle: Vec<String> = remaining.iter().map(|id| (*id).to_string()).collect();
            return Err(fail(
                TopologyErrorKind::Cycle,
                &cycle[0],
                format!("dependency cycle among: {}", cycle.join(", ")),
            ));
        }
        for id in ready {
            remaining.remove(id);
            order.push(id.to_string());
        }
    }
    Ok(order)
}

/// Transitive set of plugins that must be skipped because a dependency failed.
///
/// This is the fail-closed half of the policy: a plugin whose service is absent
/// does not start, while unrelated plugins still do.
pub fn plan_skips(manifests: &[PluginManifest], failed: &BTreeSet<PluginId>) -> BTreeSet<PluginId> {
    let mut skipped: BTreeSet<PluginId> = failed.clone();
    loop {
        let mut grew = false;
        for manifest in manifests {
            if skipped.contains(&manifest.id) {
                continue;
            }
            if manifest
                .requires
                .keys()
                .any(|service| skipped.contains(service))
            {
                skipped.insert(manifest.id.clone());
                grew = true;
            }
        }
        if !grew {
            return skipped;
        }
    }
}

fn index_manifests(manifests: &[PluginManifest]) -> Result<BTreeMap<&str, &PluginManifest>> {
    let mut index: BTreeMap<&str, &PluginManifest> = BTreeMap::new();
    for manifest in manifests {
        manifest.validate()?;
        if index.insert(manifest.id.as_str(), manifest).is_some() {
            return Err(fail(
                TopologyErrorKind::DuplicateId,
                &manifest.id,
                "the same plugin id is declared twice".to_string(),
            ));
        }
    }
    Ok(index)
}

fn fail(kind: TopologyErrorKind, plugin: &str, detail: String) -> ArkError {
    ArkError::validation(format!("{plugin}: {detail}")).with_detail(kind.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::PluginKind;

    fn manifest(id: &str, version: &str, requires: &[(&str, &str)]) -> PluginManifest {
        let mut manifest = PluginManifest::new(id, version, PluginKind::Tool).unwrap();
        for (service, req) in requires {
            manifest
                .requires
                .insert((*service).to_string(), (*req).to_string());
        }
        manifest
    }

    #[test]
    fn dependencies_come_before_dependents() {
        let manifests = vec![
            manifest("ark.app", "1.0.0", &[("ark.http.service", "^1")]),
            manifest("ark.http.service", "1.4.0", &[]),
        ];
        let order = resolve_order(&manifests).unwrap();
        assert_eq!(order, vec!["ark.http.service", "ark.app"]);
    }

    #[test]
    fn order_is_deterministic_for_independent_plugins() {
        let manifests = vec![
            manifest("ark.c", "1.0.0", &[]),
            manifest("ark.a", "1.0.0", &[]),
            manifest("ark.b", "1.0.0", &[]),
        ];
        assert_eq!(
            resolve_order(&manifests).unwrap(),
            vec!["ark.a", "ark.b", "ark.c"]
        );
    }

    #[test]
    fn a_dependency_cycle_fails_before_startup() {
        let manifests = vec![
            manifest("ark.a", "1.0.0", &[("ark.b", "^1")]),
            manifest("ark.b", "1.0.0", &[("ark.a", "^1")]),
        ];
        let err = resolve_order(&manifests).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("plugin_dependency_cycle"));
    }

    #[test]
    fn a_self_dependency_is_a_cycle() {
        let manifests = vec![manifest("ark.a", "1.0.0", &[("ark.a", "^1")])];
        let err = resolve_order(&manifests).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("plugin_dependency_cycle"));
    }

    #[test]
    fn a_missing_service_fails_before_startup() {
        let manifests = vec![manifest("ark.app", "1.0.0", &[("ark.missing", "^1")])];
        let err = resolve_order(&manifests).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("plugin_missing_dependency"));
    }

    #[test]
    fn a_major_version_mismatch_fails_before_startup() {
        let manifests = vec![
            manifest("ark.app", "1.0.0", &[("ark.obs", "^2")]),
            manifest("ark.obs", "1.9.0", &[]),
        ];
        let err = resolve_order(&manifests).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("plugin_version_incompatible"));
        assert!(err.message.contains("1.9.0"));
    }

    #[test]
    fn duplicate_ids_fail_before_startup() {
        let manifests = vec![
            manifest("ark.a", "1.0.0", &[]),
            manifest("ark.a", "2.0.0", &[]),
        ];
        let err = resolve_order(&manifests).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("plugin_duplicate_id"));
    }

    #[test]
    fn skips_are_transitive_and_leave_unrelated_plugins_alone() {
        let manifests = vec![
            manifest("ark.log.service", "1.0.0", &[]),
            manifest("ark.im", "1.0.0", &[("ark.log.service", "^1")]),
            manifest("ark.report", "1.0.0", &[("ark.im", "^1")]),
            manifest("ark.standalone", "1.0.0", &[]),
        ];
        let failed: BTreeSet<PluginId> = ["ark.log.service".to_string()].into_iter().collect();
        let skipped = plan_skips(&manifests, &failed);
        assert!(skipped.contains("ark.im"));
        assert!(skipped.contains("ark.report"), "skip must be transitive");
        assert!(!skipped.contains("ark.standalone"));
    }

    #[test]
    fn an_empty_manifest_set_resolves_to_an_empty_order() {
        assert!(resolve_order(&[]).unwrap().is_empty());
    }
}
