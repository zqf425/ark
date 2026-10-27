//! Cross-process exclusive lock over one data directory (design §7.4.3).
//!
//! `events.jsonl` and `children/runs.jsonl` are the authoritative records, and
//! sequence numbers are allocated per thread directory. Two `arkd` processes
//! pointing at the same `data_dir` would each start from the same watermark and
//! hand out duplicate `seq` values, so the second one must not start at all.
//!
//! The lock is an OS-level advisory lock on `data/.lock` held for the lifetime
//! of [`DataLock`]. It is released automatically when the process exits (even
//! on a crash), so a killed process never leaves the directory unusable.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use ark_common::{ArkError, ArkErrorCode, Result};

/// Exclusive ownership of one data directory.
///
/// Held by `FileSessionStore`; dropping the store releases the lock.
#[derive(Debug)]
pub struct DataLock {
    /// The handle owns the lock: closing it releases the OS lock.
    ///
    /// Never read after construction, but it must stay alive, hence the field
    /// and the `allow(dead_code)`.
    #[allow(dead_code)]
    file: File,
    path: PathBuf,
}

impl DataLock {
    /// Take the exclusive lock, or explain which process already holds it.
    ///
    /// `open` must be treated as a real failure rather than a warning: starting
    /// a second writer is exactly the corruption this lock exists to prevent.
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            crate::store::fs_util::ensure_dir(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|err| {
                ArkError::new(
                    ArkErrorCode::StoreFailure,
                    format!("cannot open data lock `{}`: {err}", path.display()),
                )
            })?;

        match file.try_lock() {
            Ok(()) => Ok(Self {
                file,
                path: path.to_path_buf(),
            }),
            Err(err) => {
                // `try_lock` reports contention as `WouldBlock`; anything else is
                // an environment problem (read-only mount, unsupported FS).
                let reason = match &err {
                    std::fs::TryLockError::WouldBlock => {
                        "another arkd process is already using this data directory"
                    }
                    std::fs::TryLockError::Error(_) => {
                        "the filesystem refused an exclusive lock on this data directory"
                    }
                };
                Err(ArkError::new(
                    ArkErrorCode::Conflict,
                    format!("{reason}: `{}`", path.display()),
                )
                .with_detail(err.to_string()))
            }
        }
    }

    /// Path of the lock file (diagnostics only).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_lock(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ark-data-lock-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir is creatable");
        dir.join(crate::domain::layout::names::LOCK_FILE)
    }

    #[test]
    fn a_second_acquire_on_the_same_directory_is_refused() {
        let path = temp_lock("exclusive");
        let first = DataLock::acquire(&path).expect("the first holder wins");
        let err = DataLock::acquire(&path).expect_err("a second writer must be refused");
        assert_eq!(err.code, ArkErrorCode::Conflict);
        assert!(
            err.message.contains("already using this data directory"),
            "{}",
            err.message
        );
        drop(first);
    }

    #[test]
    fn dropping_the_lock_frees_the_directory() {
        let path = temp_lock("release");
        let first = DataLock::acquire(&path).expect("the first holder wins");
        drop(first);
        // The OS lock went with the handle, so the next open succeeds.
        let second = DataLock::acquire(&path).expect("the lock must be reusable after drop");
        assert!(second
            .path()
            .ends_with(crate::domain::layout::names::LOCK_FILE));
    }
}
