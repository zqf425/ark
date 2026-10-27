//! JSON document persistence for the runtime configuration (design §7.15.3).
//!
//! A missing file is not an error: the caller gets defaults. A *corrupt* file is
//! an error, because silently resetting a user's config would be worse than
//! refusing to start.

use crate::domain::RuntimeConfig;
use std::path::{Path, PathBuf};

/// `data/config.json` under `data_dir`.
pub fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

/// Load the config file, falling back to defaults when it does not exist yet.
pub fn load_or_default(path: &Path) -> Result<RuntimeConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let config: RuntimeConfig = serde_json::from_str(&raw)
                .map_err(|err| format!("{} is not valid config JSON: {err}", path.display()))?;
            config.validate()?;
            Ok(config)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(RuntimeConfig::default()),
        Err(err) => Err(format!("cannot read {}: {err}", path.display())),
    }
}

/// Write the config atomically (temp file + rename), creating the directory.
pub fn save(path: &Path, config: &RuntimeConfig) -> Result<(), String> {
    config.validate()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(config)
        .map_err(|err| format!("cannot encode config: {err}"))?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, body).map_err(|err| format!("cannot write {}: {err}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|err| format!("cannot replace {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ark-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_yields_defaults() {
        let dir = temp_dir("missing");
        let config = load_or_default(&config_path(&dir)).unwrap();
        assert_eq!(config, RuntimeConfig::default());
    }

    #[test]
    fn roundtrip_preserves_fields() {
        let dir = temp_dir("roundtrip");
        let path = config_path(&dir);
        let mut config = RuntimeConfig::default();
        config.log_level = "debug".into();
        config.tuning.max_rounds = 7;
        save(&path, &config).unwrap();
        assert_eq!(load_or_default(&path).unwrap(), config);
    }

    #[test]
    fn corrupt_file_is_an_error_not_a_reset() {
        let dir = temp_dir("corrupt");
        let path = config_path(&dir);
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(load_or_default(&path).is_err());
    }
}
