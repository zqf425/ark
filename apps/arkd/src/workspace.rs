//! Live workspace root resolution (design §7.15).
//!
//! `default_workspace` is a `Hot` config field, so a value chosen in Settings has
//! to apply to the *next* tool call, not the next process start. The tool runner
//! therefore asks for the root on every call instead of capturing it once.
//!
//! Two sources, in order:
//!
//! 1. `config.default_workspace` — what the settings screen writes;
//! 2. the CLI `--workspace` value — the fallback, so a runtime started without a
//!    saved setting still has somewhere to work.
//!
//! A path that no longer exists falls back as well: a deleted folder must not
//! make every tool call fail with a confusing "not found" from deep inside a tool.

use std::path::PathBuf;
use std::sync::Arc;

use ark_config::ConfigStore;

/// Resolves the workspace root that tools should operate in.
pub struct WorkspaceRoots {
    config: Arc<ConfigStore>,
    fallback: PathBuf,
}

impl WorkspaceRoots {
    pub fn new(config: Arc<ConfigStore>, fallback: PathBuf) -> Self {
        Self { config, fallback }
    }

    /// The root to use for a tool call starting now.
    pub fn current(&self) -> PathBuf {
        let configured = self
            .config
            .snapshot()
            .default_workspace
            .as_ref()
            .filter(|path| path.is_dir())
            .cloned();
        configured.unwrap_or_else(|| self.fallback.clone())
    }

    /// The CLI fallback, regardless of configuration.
    pub fn fallback(&self) -> &PathBuf {
        &self.fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_config::RuntimeConfig;

    fn roots(default: Option<PathBuf>, fallback: PathBuf) -> WorkspaceRoots {
        let mut config = RuntimeConfig::default();
        config.default_workspace = default;
        WorkspaceRoots::new(Arc::new(ConfigStore::new(config)), fallback)
    }

    #[test]
    fn falls_back_when_no_default_is_configured() {
        let fallback = std::env::temp_dir();
        let roots = roots(None, fallback.clone());
        assert_eq!(roots.current(), fallback);
    }

    #[test]
    fn uses_the_configured_default_when_it_exists() {
        let dir = std::env::temp_dir();
        let roots = roots(Some(dir.clone()), PathBuf::from("D:/does-not-matter"));
        assert_eq!(roots.current(), dir);
    }

    #[test]
    fn falls_back_when_the_configured_default_is_missing() {
        // A deleted folder must not turn every tool call into a confusing error.
        let missing = std::env::temp_dir().join("ark-roots-does-not-exist-xyz");
        let fallback = std::env::temp_dir();
        let roots = roots(Some(missing), fallback.clone());
        assert_eq!(roots.current(), fallback);
    }

    #[test]
    fn a_config_change_is_visible_on_the_next_call() {
        // This is the property the settings screen depends on: no restart.
        let dir = std::env::temp_dir();
        let mut config = RuntimeConfig::default();
        config.default_workspace = None;
        let store = Arc::new(ConfigStore::new(config));
        let roots = WorkspaceRoots::new(Arc::clone(&store), PathBuf::from("D:/fallback"));
        assert_eq!(roots.current(), PathBuf::from("D:/fallback"));

        let patch = ark_contract::ConfigPatch {
            default_workspace: Some(dir.to_string_lossy().to_string()),
            ..ark_contract::ConfigPatch::default()
        };
        assert!(matches!(
            store.apply(patch),
            ark_contract::ApplyResult::Applied { .. }
        ));
        assert_eq!(roots.current(), dir);
    }
}
