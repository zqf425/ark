//! Runtime-owned directory layout.
//!
//! The session store keeps accepting an explicit `--data-dir`. When that path
//! is the conventional `<root>/data`, sibling services use the same root;
//! otherwise the explicit directory itself remains the compatibility root.

use std::path::{Path, PathBuf};

pub const LOG_FILE_NAME: &str = "arkd.log";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    pub root: PathBuf,
    pub data: PathBuf,
    pub config: PathBuf,
    pub log: PathBuf,
    pub skills: PathBuf,
    pub mcp: PathBuf,
    pub tasks: PathBuf,
    pub workspace: PathBuf,
    pub im: PathBuf,
}

impl RuntimePaths {
    pub fn from_data_dir(data_dir: &Path) -> Self {
        let root = if data_dir.file_name().is_some_and(|name| name == "data") {
            data_dir
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        } else {
            data_dir.to_path_buf()
        };
        Self {
            data: data_dir.to_path_buf(),
            config: root.join("config"),
            log: root.join("log"),
            skills: root.join("skills"),
            mcp: root.join("mcp"),
            tasks: root.join("tasks"),
            workspace: root.join("workspace"),
            im: root.join("IM"),
            root,
        }
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for path in [
            &self.root,
            &self.data,
            &self.config,
            &self.log,
            &self.skills,
            &self.mcp,
            &self.tasks,
            &self.workspace,
            &self.im,
        ] {
            std::fs::create_dir_all(path)?;
        }
        Ok(())
    }

    pub fn log_file(&self) -> PathBuf {
        self.log.join(LOG_FILE_NAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conventional_data_dir_uses_sibling_directories() {
        let paths = RuntimePaths::from_data_dir(Path::new("/home/user/.arkd/data"));
        assert_eq!(paths.root, PathBuf::from("/home/user/.arkd"));
        assert_eq!(paths.config, PathBuf::from("/home/user/.arkd/config"));
        assert_eq!(paths.im, PathBuf::from("/home/user/.arkd/IM"));
        assert_eq!(paths.workspace, PathBuf::from("/home/user/.arkd/workspace"));
    }

    #[test]
    fn custom_data_dir_remains_a_self_contained_compatibility_root() {
        let paths = RuntimePaths::from_data_dir(Path::new("/tmp/ark-custom"));
        assert_eq!(paths.root, PathBuf::from("/tmp/ark-custom"));
        assert_eq!(paths.data, PathBuf::from("/tmp/ark-custom"));
        assert_eq!(paths.config, PathBuf::from("/tmp/ark-custom/config"));
        assert_eq!(paths.tasks, PathBuf::from("/tmp/ark-custom/tasks"));
        assert_eq!(paths.im, PathBuf::from("/tmp/ark-custom/IM"));
    }

    #[test]
    fn relative_data_directory_uses_current_directory_as_root() {
        let paths = RuntimePaths::from_data_dir(Path::new("data"));
        assert_eq!(paths.root, PathBuf::from("."));
        assert_eq!(paths.config, PathBuf::from("./config"));
    }

    #[test]
    fn ensure_creates_every_category_with_exact_im_case() {
        let temp = tempfile::tempdir().expect("temp dir");
        let paths = RuntimePaths::from_data_dir(&temp.path().join("data"));
        paths.ensure().expect("create layout");
        for path in [
            &paths.data,
            &paths.config,
            &paths.log,
            &paths.skills,
            &paths.mcp,
            &paths.tasks,
            &paths.workspace,
            &paths.im,
        ] {
            assert!(path.is_dir(), "missing {}", path.display());
        }
        assert!(temp.path().join("IM").is_dir());
    }
}
