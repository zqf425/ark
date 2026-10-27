//! Filesystem helpers shared by the store.
//!
//! Every mutation that must be crash-safe goes through `write_atomic`
//! (temp file + rename), so a torn write can never leave a half-updated
//! `thread.json`, `session.json` or `index.json`.

use ark_common::{ArkError, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Create the directory and every missing parent.
pub fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|err| io_error("create directory", path, err))
}

/// Create the directory and every missing parent (async).
pub async fn ensure_dir_async(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|err| io_error("create directory", path, err))
}

/// Write `bytes` to `path` atomically: unique temp file, fsync, rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let temp = temp_sibling(path);
    {
        use std::io::Write;
        let mut file =
            std::fs::File::create(&temp).map_err(|err| io_error("create temp file", &temp, err))?;
        file.write_all(bytes)
            .map_err(|err| io_error("write temp file", &temp, err))?;
        file.sync_all()
            .map_err(|err| io_error("sync temp file", &temp, err))?;
    }
    std::fs::rename(&temp, path).map_err(|err| io_error("rename into place", path, err))
}

/// Serialize `value` as pretty JSON and write it atomically.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    write_atomic(path, &bytes)
}

/// Write `value` as pretty JSON only when the on-disk bytes differ.
///
/// Derived files (children/index.json, index.json) are rewritten on every pass;
/// skipping an identical rewrite keeps mtimes stable and avoids pointless IO.
pub fn save_json_if_changed<T: Serialize>(path: &Path, value: &T) -> Result<bool> {
    let bytes = serde_json::to_vec_pretty(value)?;
    match std::fs::read(path) {
        Ok(existing) if existing == bytes => Ok(false),
        _ => {
            write_atomic(path, &bytes)?;
            Ok(true)
        }
    }
}
/// Read and parse JSON, mapping every failure to `None`.
///
/// Decoding follows the shared policy (UTF-8 first, BOM stripped, documented
/// fallbacks), so a config or index file touched by a Windows editor still loads.
pub fn read_json_opt<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let raw = ark_common::read_text_opt(path)?;
    serde_json::from_str(&raw).ok()
}

/// Append one line (caller includes the trailing newline) and flush.
pub fn append_line_blocking(path: &Path, line: &str, fsync: bool) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| io_error("open for append", path, err))?;
    file.write_all(line.as_bytes())
        .map_err(|err| io_error("append", path, err))?;
    file.flush().map_err(|err| io_error("flush", path, err))?;
    if fsync {
        file.sync_all()
            .map_err(|err| io_error("fsync", path, err))?;
    }
    Ok(())
}

/// Whether `path` exists as a file.
pub fn is_file(path: &Path) -> bool {
    path.is_file()
}

/// Length in bytes, or zero when the file is absent.
pub fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// Remove a directory tree; missing directories are not an error.
pub fn remove_dir_all(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io_error("remove directory", path, err)),
    }
}

/// Rename a directory (used for `children.deleting-<uuid>`).
pub fn rename_dir(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        ensure_dir(parent)?;
    }
    std::fs::rename(from, to).map_err(|err| io_error("rename directory", from, err))
}

/// Sorted directory entries; a missing directory yields an empty list.
pub fn list_dirs(path: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
        .map(|iter| {
            iter.filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let temp_name = format!(".{name}.{}.tmp", uuid::Uuid::new_v4());
    match path.parent() {
        Some(parent) => parent.join(temp_name),
        None => PathBuf::from(temp_name),
    }
}

fn io_error(action: &str, path: &Path, err: std::io::Error) -> ArkError {
    ArkError::store(format!("{action} failed for {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::TempDir;

    #[test]
    fn atomic_write_replaces_content_without_leftovers() {
        let dir = TempDir::new("fs-util-atomic");
        let path = dir.path().join("nested/file.json");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        let entries: Vec<_> = std::fs::read_dir(dir.path().join("nested"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(entries.is_empty(), "temp files must be renamed away");
    }

    #[test]
    fn json_roundtrip_and_missing_file() {
        let dir = TempDir::new("fs-util-json");
        let path = dir.path().join("index.json");
        assert!(read_json_opt::<serde_json::Value>(&path).is_none());
        write_json_atomic(&path, &serde_json::json!({"a": 1})).unwrap();
        let value: serde_json::Value = read_json_opt(&path).unwrap();
        assert_eq!(value["a"], 1);
    }

    #[test]
    fn remove_missing_directory_is_not_an_error() {
        let dir = TempDir::new("fs-util-remove");
        remove_dir_all(&dir.path().join("absent")).unwrap();
    }

    #[test]
    fn rename_directory_moves_the_tree() {
        let dir = TempDir::new("fs-util-rename");
        let from = dir.path().join("children");
        ensure_dir(&from).unwrap();
        write_atomic(&from.join("index.json"), b"{}").unwrap();
        let to = dir.path().join("children.deleting-1");
        rename_dir(&from, &to).unwrap();
        assert!(!from.exists());
        assert!(to.join("index.json").is_file());
    }

    #[test]
    fn list_dirs_returns_sorted_directories_only() {
        let dir = TempDir::new("fs-util-list");
        ensure_dir(&dir.path().join("b")).unwrap();
        ensure_dir(&dir.path().join("a")).unwrap();
        write_atomic(&dir.path().join("file.txt"), b"x").unwrap();
        let names: Vec<String> = list_dirs(dir.path())
            .into_iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }
}
