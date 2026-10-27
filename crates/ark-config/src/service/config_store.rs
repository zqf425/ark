//! `ConfigStore`: the single mutation point for runtime configuration (§7.15.3).
//!
//! Reads are lock-free from the caller's point of view: they clone an
//! `Arc<ConfigSnapshot>`, which is what makes "apply then read" monotonic. Writes
//! serialize behind a mutex, validate the candidate first, and only then swap,
//! so a rejected patch leaves every field byte-identical.

use ark_contract::{ApplyResult, ConfigPatch, FieldPath, ReloadClass};
use std::sync::{Arc, Mutex, PoisonError};

use crate::domain::{reload_class, restart_fields, tuning_diff, RuntimeConfig};

/// One immutable configuration version.
#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    version: u64,
    config: Arc<RuntimeConfig>,
}

impl ConfigSnapshot {
    /// Monotonic version. Increments once per applied patch.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }
}

/// Atomic configuration holder.
#[derive(Debug)]
pub struct ConfigStore {
    inner: Mutex<ConfigSnapshot>,
}

impl ConfigStore {
    pub fn new(config: RuntimeConfig) -> Self {
        Self {
            inner: Mutex::new(ConfigSnapshot {
                version: 1,
                config: Arc::new(config),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ConfigSnapshot> {
        // A poisoned lock only means another thread panicked while holding it;
        // the snapshot itself is still a consistent `Arc`, so recover it.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Current snapshot. Cheap: one `Arc` clone.
    pub fn snapshot(&self) -> Arc<RuntimeConfig> {
        self.lock().config.clone()
    }

    pub fn version(&self) -> u64 {
        self.lock().version
    }

    /// Reject a patch that touches `Restart` fields, as a whole batch.
    pub fn check_restart(&self, patch: &ConfigPatch) -> Option<ApplyResult> {
        let restart = restart_fields(patch);
        if restart.is_empty() {
            return None;
        }
        Some(ApplyResult::Rejected {
            restart_required: restart,
            reason: "patch touches a field that requires an arkd restart".to_string(),
        })
    }

    /// Validate then swap. `Restart` fields are refused before validation so the
    /// rejection reason always names the real blocker.
    pub fn apply(&self, patch: ConfigPatch) -> ApplyResult {
        if let Some(rejected) = self.check_restart(&patch) {
            return rejected;
        }
        let mut guard = self.lock();
        let current = guard.config.clone();
        let (candidate, coarse) = match current.candidate(&patch) {
            Ok(value) => value,
            Err(reason) => {
                return ApplyResult::Rejected {
                    restart_required: Vec::new(),
                    reason,
                }
            }
        };
        if let Err(reason) = candidate.validate() {
            return ApplyResult::Rejected {
                restart_required: Vec::new(),
                reason,
            };
        }
        let changed = refine_paths(&coarse, &current, &candidate);
        if changed.is_empty() {
            // A patch that names no real change must not mint a new version: the
            // version identifies a *configuration state*, and bumping it on a
            // no-op would make subscribers recompute for nothing and would hide
            // the difference between "applied" and "nothing to do".
            return ApplyResult::Applied {
                version: guard.version,
                changed: Vec::new(),
            };
        }
        let version = guard.version + 1;
        // The only mutation: one pointer swap of a fully validated snapshot.
        guard.version = version;
        guard.config = Arc::new(candidate);
        ApplyResult::Applied { version, changed }
    }

    /// Replace the whole configuration (startup / file reload). Still validated.
    pub fn replace(&self, config: RuntimeConfig) -> Result<u64, String> {
        config.validate()?;
        let mut guard = self.lock();
        guard.version += 1;
        guard.config = Arc::new(config);
        Ok(guard.version)
    }
}

/// Expand coarse `tuning` into the exact nested paths that changed.
fn refine_paths(coarse: &[FieldPath], old: &RuntimeConfig, new: &RuntimeConfig) -> Vec<FieldPath> {
    let mut out = Vec::new();
    for path in coarse {
        if path.0 == "tuning" {
            out.extend(tuning_diff(&old.tuning, &new.tuning));
        } else {
            out.push(path.clone());
        }
    }
    out
}

/// Reload class of every path a change reported, in report order.
pub fn classes_of(paths: &[FieldPath]) -> Vec<ReloadClass> {
    paths.iter().map(|path| reload_class(&path.0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TuningConfig;

    fn store() -> ConfigStore {
        ConfigStore::new(RuntimeConfig::default())
    }

    #[test]
    fn hot_patch_applies_and_bumps_the_version() {
        let store = store();
        let before = store.version();
        let result = store.apply(ConfigPatch {
            log_level: Some("debug".into()),
            ..ConfigPatch::default()
        });
        match result {
            ApplyResult::Applied { version, changed } => {
                assert_eq!(version, before + 1);
                assert_eq!(changed, vec![FieldPath::new("log_level")]);
            }
            other => panic!("expected applied, got {other:?}"),
        }
        assert_eq!(store.snapshot().log_level, "debug");
        assert_eq!(store.version(), before + 1);
    }

    #[test]
    fn invalid_patch_leaves_every_field_untouched() {
        let store = store();
        let before = store.snapshot();
        let result = store.apply(ConfigPatch {
            log_level: Some("shout".into()),
            ..ConfigPatch::default()
        });
        assert!(matches!(result, ApplyResult::Rejected { .. }));
        let after = store.snapshot();
        assert_eq!(after.as_ref(), before.as_ref(), "no field may change");
        assert_eq!(store.version(), 1, "a rejected patch is not a version");
    }

    #[test]
    fn restart_field_rejects_the_whole_batch() {
        let store = store();
        let before = store.snapshot();
        let result = store.apply(ConfigPatch {
            port: Some(19100),
            log_level: Some("trace".into()),
            ..ConfigPatch::default()
        });
        match result {
            ApplyResult::Rejected {
                restart_required, ..
            } => {
                assert_eq!(restart_required, vec![FieldPath::new("port")]);
            }
            other => panic!("expected rejected, got {other:?}"),
        }
        // The Hot field in the same batch must not have been applied either.
        assert_eq!(store.snapshot().log_level, before.log_level);
        assert_eq!(store.snapshot().port, before.port);
        assert_eq!(store.version(), 1);
    }

    #[test]
    fn tuning_component_is_reported_as_nested_paths() {
        let store = store();
        let mut tuning = TuningConfig::default();
        tuning.max_rounds = 5;
        tuning.chunk_buffer = 8;
        let result = store.apply(ConfigPatch {
            tuning: Some(serde_json::to_value(&tuning).unwrap()),
            ..ConfigPatch::default()
        });
        match result {
            ApplyResult::Applied { changed, .. } => {
                assert!(changed.contains(&FieldPath::new("tuning.max_rounds")));
                assert!(changed.contains(&FieldPath::new("tuning.chunk_buffer")));
                assert_eq!(changed.len(), 2);
            }
            other => panic!("expected applied, got {other:?}"),
        }
        assert_eq!(store.snapshot().tuning.max_rounds, 5);
    }

    #[test]
    fn out_of_range_tuning_is_rejected_without_a_swap() {
        let store = store();
        let mut tuning = TuningConfig::default();
        tuning.max_rounds = 0;
        let result = store.apply(ConfigPatch {
            tuning: Some(serde_json::to_value(&tuning).unwrap()),
            ..ConfigPatch::default()
        });
        assert!(matches!(result, ApplyResult::Rejected { .. }));
        assert_eq!(
            store.snapshot().tuning.max_rounds,
            TuningConfig::default().max_rounds
        );
    }

    #[test]
    fn empty_patch_is_a_no_op_version_bump() {
        let store = store();
        let result = store.apply(ConfigPatch::default());
        match result {
            ApplyResult::Applied { changed, .. } => assert!(changed.is_empty()),
            other => panic!("expected applied, got {other:?}"),
        }
    }

    #[test]
    fn classes_match_the_classification_table() {
        let classes = classes_of(&[FieldPath::new("log_level"), FieldPath::new("port")]);
        assert_eq!(classes, vec![ReloadClass::Hot, ReloadClass::Restart]);
    }
}
