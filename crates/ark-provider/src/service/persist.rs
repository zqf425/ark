//! Provider table persistence (design §7.2).
//!
//! Reads go through `ark_common::read_json_file_opt` (UTF-8 first, BOM stripped)
//! and writes through `ark_common::write_json_file` (temp file + rename, never a
//! BOM). `std::fs::read_to_string` is deliberately unused: on this project's
//! Windows hosts it decodes with the ANSI code page and mangles or fails on a
//! BOM-prefixed file.
//!
//! The secret is stored in the same file as the rest of the entry. That is a v1
//! simplification with the same permissions as ordinary configuration; the
//! follow-up is to move `api_key` into the OS keyring (Windows Credential
//! Manager / macOS Keychain) and keep only the reference here.
//!
//! [`read_document`] returns `None` for both a missing and an unreadable file.
//! The distinction is reported through [`DocumentLoad`] so the caller can warn
//! instead of silently resetting a user's table.

use std::path::Path;

use ark_common::{read_json_file_opt, write_json_file, Result};

use crate::domain::ProvidersDocument;

/// Outcome of loading the table from disk.
#[derive(Debug)]
pub enum DocumentLoad {
    /// The file exists and parsed.
    Loaded(ProvidersDocument),
    /// No file yet: a fresh install.
    Missing,
    /// The file exists but is not valid JSON, or cannot be read.
    Unusable(String),
}

/// Load `providers.json`, tolerating a BOM and reporting a corrupt file.
pub fn load_document(path: &Path) -> DocumentLoad {
    if !path.exists() {
        return DocumentLoad::Missing;
    }
    match read_json_file_opt::<ProvidersDocument>(path) {
        Some(document) => DocumentLoad::Loaded(document),
        None => DocumentLoad::Unusable(format!(
            "{} is not a valid provider table; starting from an empty table",
            path.display()
        )),
    }
}

/// Read the table, defaulting to an empty document when absent or unreadable.
pub fn read_document(path: &Path) -> ProvidersDocument {
    match load_document(path) {
        DocumentLoad::Loaded(document) => document,
        DocumentLoad::Missing | DocumentLoad::Unusable(_) => ProvidersDocument::default(),
    }
}

/// Persist the table atomically: UTF-8, no BOM, temp file + rename.
pub fn write_document(path: &Path, document: &ProvidersDocument) -> Result<()> {
    write_json_file(path, document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::ProviderEntry;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ark-provider-persist-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_file_is_reported_as_missing_not_as_corrupt() {
        let dir = temp_dir("missing");
        let path = dir.join("providers.json");
        assert!(matches!(load_document(&path), DocumentLoad::Missing));
        assert!(read_document(&path).providers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_reported_as_unusable() {
        let dir = temp_dir("corrupt");
        let path = dir.join("providers.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(matches!(load_document(&path), DocumentLoad::Unusable(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_creates_parent_directories_without_a_bom() {
        let dir = temp_dir("nested");
        let path = dir.join("deeper").join("providers.json");
        let mut document = ProvidersDocument::default();
        document.put(ProviderEntry::new("deepseek", "deepseek-chat"));
        write_document(&path, &document).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.starts_with(&ark_common::UTF8_BOM_BYTES));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
