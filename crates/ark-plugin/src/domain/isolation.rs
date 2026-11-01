//! Fault isolation for the local plugin directory (design 7.7.3).
//!
//! [`crate::domain::topology`] deliberately aborts the whole load on a
//! declaration error, because a compiled plugin with a broken manifest is a
//! build bug that must not be papered over. Directory plugins are different:
//! they are third-party files a user dropped into `data/plugins`, so one bad
//! declaration must not stop the runtime.
//!
//! This module is the pure half of that policy. It partitions a manifest set
//! into what can start and what must be dropped, and it explains every drop.
//! Nothing here touches the filesystem, so the partition rules are unit-testable
//! and the loader stays a thin I/O shell.

use ark_common::Result;
use std::collections::{BTreeMap, BTreeSet};

use crate::domain::manifest::{PluginManifest, VersionReq};
use crate::domain::topology::{plan_skips, resolve_order, PluginId};

/// Why one plugin was dropped before its code ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDrop {
    pub plugin_id: PluginId,
    /// Stable machine code, e.g. `plugin_dependency_cycle`.
    pub code: String,
    pub detail: String,
}

/// What the loader should do with a scanned manifest set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IsolationPlan {
    /// Loadable plugins in dependency order.
    pub ready: Vec<PluginId>,
    /// Dropped plugins, each with a diagnostic code.
    pub dropped: Vec<PluginDrop>,
}

/// Partition `manifests` into a loadable order plus explained drops.
///
/// Missing dependencies and major-version mismatches are removed first, then
/// dependency cycles, repeating until the remaining set resolves cleanly. A
/// plugin whose dependency was dropped goes with it, transitively.
pub fn plan_local_load(manifests: &[PluginManifest]) -> Result<IsolationPlan> {
    let mut active: Vec<PluginManifest> = manifests.to_vec();
    let mut dropped: Vec<PluginDrop> = Vec::new();

    loop {
        let unmet = unmet_requirements(&active);
        if !unmet.is_empty() {
            drop_with_dependents(&mut active, &mut dropped, unmet);
            continue;
        }
        let cycle = cycle_members(&active);
        if !cycle.is_empty() {
            let names: Vec<String> = cycle.iter().cloned().collect();
            for plugin_id in &cycle {
                dropped.push(PluginDrop {
                    plugin_id: plugin_id.clone(),
                    code: "plugin_dependency_cycle".to_string(),
                    detail: format!("dependency cycle among: {}", names.join(", ")),
                });
            }
            active.retain(|manifest| !cycle.contains(&manifest.id));
            continue;
        }
        break;
    }

    Ok(IsolationPlan {
        ready: resolve_order(&active)?,
        dropped,
    })
}

/// First unmet `requires` entry of every manifest that has one.
fn unmet_requirements(manifests: &[PluginManifest]) -> BTreeMap<PluginId, (String, String)> {
    let versions: BTreeMap<&str, &crate::domain::manifest::Version> = manifests
        .iter()
        .map(|manifest| (manifest.id.as_str(), &manifest.version))
        .collect();
    let mut unmet = BTreeMap::new();
    for manifest in manifests {
        for (service, requirement) in &manifest.requires {
            let Some(installed) = versions.get(service.as_str()) else {
                unmet.insert(
                    manifest.id.clone(),
                    (
                        "plugin_missing_dependency".to_string(),
                        format!("requires {service} {requirement} but it is not installed"),
                    ),
                );
                break;
            };
            let compatible = VersionReq::parse(requirement)
                .map(|req| req.accepts(installed))
                .unwrap_or(false);
            if !compatible {
                unmet.insert(
                    manifest.id.clone(),
                    (
                        "plugin_version_incompatible".to_string(),
                        format!(
                            "requires {service} {requirement} but installed version is {installed}"
                        ),
                    ),
                );
                break;
            }
        }
    }
    unmet
}

/// Drop the unmet plugins and, transitively, everything that requires them.
fn drop_with_dependents(
    active: &mut Vec<PluginManifest>,
    dropped: &mut Vec<PluginDrop>,
    unmet: BTreeMap<PluginId, (String, String)>,
) {
    let seeds: BTreeSet<PluginId> = unmet.keys().cloned().collect();
    let affected = plan_skips(active, &seeds);
    let mut ordered: Vec<&PluginManifest> =
        active.iter().filter(|m| affected.contains(&m.id)).collect();
    ordered.sort_by(|a, b| a.id.cmp(&b.id));
    for manifest in ordered {
        let (code, detail) = match unmet.get(&manifest.id) {
            Some((code, detail)) => (code.clone(), detail.clone()),
            None => (
                "plugin_skipped".to_string(),
                "a required plugin failed to load".to_string(),
            ),
        };
        dropped.push(PluginDrop {
            plugin_id: manifest.id.clone(),
            code,
            detail,
        });
    }
    active.retain(|manifest| !affected.contains(&manifest.id));
}

/// Plugins that sit on a dependency cycle: whatever Kahn's algorithm cannot
/// drain, including plugins that only depend on a cycle.
fn cycle_members(manifests: &[PluginManifest]) -> BTreeSet<PluginId> {
    let mut remaining: BTreeSet<&str> = manifests.iter().map(|m| m.id.as_str()).collect();
    loop {
        let ready: Vec<&str> = remaining
            .iter()
            .copied()
            .filter(|id| {
                manifests
                    .iter()
                    .find(|manifest| manifest.id == *id)
                    .map(|manifest| {
                        manifest
                            .requires
                            .keys()
                            .all(|service| !remaining.contains(service.as_str()))
                    })
                    .unwrap_or(true)
            })
            .collect();
        if ready.is_empty() {
            return remaining.iter().map(|id| (*id).to_string()).collect();
        }
        for id in ready {
            remaining.remove(id);
        }
    }
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

    fn dropped_ids(plan: &IsolationPlan) -> Vec<String> {
        plan.dropped.iter().map(|d| d.plugin_id.clone()).collect()
    }

    #[test]
    fn healthy_plugins_keep_their_dependency_order() {
        let manifests = vec![
            manifest("ark.example.app", "1.0.0", &[("ark.example.base", "^1")]),
            manifest("ark.example.base", "1.4.0", &[]),
        ];
        let plan = plan_local_load(&manifests).unwrap();
        assert_eq!(plan.ready, vec!["ark.example.base", "ark.example.app"]);
        assert!(plan.dropped.is_empty());
    }

    #[test]
    fn a_cycle_is_dropped_and_healthy_plugins_still_load() {
        let manifests = vec![
            manifest("ark.example.a", "1.0.0", &[("ark.example.b", "^1")]),
            manifest("ark.example.b", "1.0.0", &[("ark.example.a", "^1")]),
            manifest("ark.example.ok", "1.0.0", &[]),
        ];
        let plan = plan_local_load(&manifests).unwrap();
        assert_eq!(plan.ready, vec!["ark.example.ok"]);
        assert_eq!(dropped_ids(&plan), vec!["ark.example.a", "ark.example.b"]);
        assert!(plan
            .dropped
            .iter()
            .all(|drop| drop.code == "plugin_dependency_cycle"));
    }

    #[test]
    fn a_missing_dependency_drops_the_dependent_only() {
        let manifests = vec![
            manifest(
                "ark.example.orphan",
                "1.0.0",
                &[("ark.example.absent", "^1")],
            ),
            manifest("ark.example.ok", "1.0.0", &[]),
        ];
        let plan = plan_local_load(&manifests).unwrap();
        assert_eq!(plan.ready, vec!["ark.example.ok"]);
        assert_eq!(
            plan.dropped[0].code, "plugin_missing_dependency",
            "the drop must explain itself"
        );
    }

    #[test]
    fn a_major_version_mismatch_drops_the_dependent() {
        let manifests = vec![
            manifest("ark.example.app", "1.0.0", &[("ark.example.base", "^2")]),
            manifest("ark.example.base", "1.9.0", &[]),
        ];
        let plan = plan_local_load(&manifests).unwrap();
        assert_eq!(plan.ready, vec!["ark.example.base"]);
        assert_eq!(plan.dropped[0].code, "plugin_version_incompatible");
    }

    #[test]
    fn dependents_of_a_dropped_plugin_are_dropped_transitively() {
        let manifests = vec![
            manifest(
                "ark.example.broken",
                "1.0.0",
                &[("ark.example.absent", "^1")],
            ),
            manifest("ark.example.mid", "1.0.0", &[("ark.example.broken", "^1")]),
            manifest("ark.example.top", "1.0.0", &[("ark.example.mid", "^1")]),
            manifest("ark.example.ok", "1.0.0", &[]),
        ];
        let plan = plan_local_load(&manifests).unwrap();
        assert_eq!(plan.ready, vec!["ark.example.ok"]);
        assert_eq!(
            dropped_ids(&plan),
            vec!["ark.example.broken", "ark.example.mid", "ark.example.top"]
        );
    }

    #[test]
    fn an_empty_set_plans_nothing() {
        let plan = plan_local_load(&[]).unwrap();
        assert!(plan.ready.is_empty());
        assert!(plan.dropped.is_empty());
    }
}
