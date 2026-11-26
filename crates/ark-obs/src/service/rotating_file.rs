use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::Local;

pub(super) const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

pub(super) struct RotatingFile {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    max_bytes: u64,
}

impl RotatingFile {
    pub(super) fn open(path: impl Into<PathBuf>, max_bytes: u64) -> io::Result<Self> {
        let path = path.into();
        let file = Self::open_active(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path,
            file: Some(file),
            size,
            max_bytes,
        })
    }

    fn open_active(path: &Path) -> io::Result<File> {
        OpenOptions::new().create(true).append(true).open(path)
    }

    fn ensure_active(&mut self) -> io::Result<()> {
        if self.file.is_none() {
            let file = Self::open_active(&self.path)?;
            // Another process may have appended while no handle was open.
            self.size = file.metadata()?.len();
            self.file = Some(file);
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.ensure_active()?;
        self.flush()?;
        let rotated = self.rotated_path();
        self.file.take();
        if let Err(error) = fs::rename(&self.path, &rotated) {
            // Preserve the rename error; a later write retries the reopen.
            let _ = self.ensure_active();
            return Err(error);
        }

        self.file = None;
        self.file = Some(Self::open_active(&self.path)?);
        self.size = 0;
        Ok(())
    }

    fn rotated_path(&self) -> PathBuf {
        let stem = self
            .path
            .file_stem()
            .unwrap_or_else(|| OsStr::new("arkd"))
            .to_string_lossy();
        let timestamp = Local::now().format("%Y%m%d-%H%M%S-%3f");
        for sequence in 0_u64.. {
            let suffix = if sequence == 0 {
                String::new()
            } else {
                format!("-{sequence}")
            };
            let name = format!("{stem}-{timestamp}{suffix}");
            let rotated = self.path.with_file_name(format!("{name}.log"));
            if !rotated.exists() {
                return rotated;
            }
        }
        unreachable!("rotation sequence always has an unused name")
    }
}

impl Write for RotatingFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.ensure_active()?;
        if self.size >= self.max_bytes {
            self.rotate()?;
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "log file is closed"))?;
        file.write_all(bytes)?;
        self.size = self.size.saturating_add(bytes.len() as u64);
        if !bytes.is_empty() && self.size >= self.max_bytes {
            self.rotate()?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "log file is closed"))?
            .flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotated_logs(directory: &Path) -> Vec<PathBuf> {
        let mut paths: Vec<_> = fs::read_dir(directory)
            .expect("read log directory")
            .map(|entry| entry.expect("log entry").path())
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "log")
                    && path.file_name().is_some_and(|name| name != "arkd.log")
            })
            .collect();
        paths.sort();
        paths
    }

    #[test]
    fn rotates_as_soon_as_the_file_reaches_the_limit() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("arkd.log");
        fs::write(&path, b"1234").expect("seed log");
        let mut writer = RotatingFile::open(&path, 8).expect("open log");

        writer.write_all(b"5678").expect("write at limit");
        writer.flush().expect("flush log");

        assert!(fs::read(&path).expect("read new log").is_empty());
        let rotated = rotated_logs(directory.path());
        assert_eq!(rotated.len(), 1);
        assert_eq!(
            fs::read(&rotated[0]).expect("read rotated log"),
            b"12345678"
        );
    }

    #[test]
    fn new_writes_continue_after_rotation() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("arkd.log");
        let mut writer = RotatingFile::open(&path, 6).expect("open log");

        writer.write_all(b"12345").expect("write below limit");
        writer.write_all(b"6").expect("trigger rotation");
        writer.write_all(b"next\n").expect("write to new log");
        writer.flush().expect("flush log");

        assert_eq!(fs::read(&path).expect("read active log"), b"next\n");
        let rotated = rotated_logs(directory.path());
        assert_eq!(rotated.len(), 1);
        assert_eq!(fs::read(&rotated[0]).expect("read rotated log"), b"123456");
    }

    #[test]
    fn an_existing_file_at_the_limit_rotates_before_the_next_write() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("arkd.log");
        fs::write(&path, b"12345678").expect("seed log");
        let mut writer = RotatingFile::open(&path, 8).expect("open log");

        writer.write_all(b"next\n").expect("write to new log");
        writer.flush().expect("flush log");

        assert_eq!(fs::read(&path).expect("read active log"), b"next\n");
        let rotated = rotated_logs(directory.path());
        assert_eq!(rotated.len(), 1);
        assert_eq!(
            fs::read(&rotated[0]).expect("read rotated log"),
            b"12345678"
        );
    }

    #[test]
    fn write_reopens_a_missing_handle_and_resets_size_from_disk() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("arkd.log");
        fs::write(&path, b"existing").expect("seed log");
        let mut writer = RotatingFile::open(&path, 64).expect("open log");
        writer.file = None;
        writer.size = u64::MAX;

        writer
            .write_all(b"-next")
            .expect("write reopens active log");
        writer.flush().expect("flush reopened log");

        assert_eq!(fs::read(&path).expect("read active log"), b"existing-next");
        assert_eq!(writer.size, b"existing-next".len() as u64);
        assert!(rotated_logs(directory.path()).is_empty());
    }

    #[test]
    fn rotated_name_uses_the_current_local_time() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("arkd.log");
        let mut writer = RotatingFile::open(&path, 1).expect("open log");

        writer.write_all(b"at limit").expect("trigger rotation");

        let rotated = rotated_logs(directory.path());
        assert_eq!(rotated.len(), 1);
        let file_name = rotated[0]
            .file_name()
            .expect("rotated name")
            .to_string_lossy();
        let timestamp = file_name
            .strip_prefix("arkd-")
            .and_then(|name| name.strip_suffix(".log"))
            .expect("timestamp log name");
        chrono::NaiveDateTime::parse_from_str(timestamp, "%Y%m%d-%H%M%S-%3f")
            .expect("rotated name contains a local timestamp");
    }

    #[test]
    fn repeated_rotations_use_distinct_names() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("arkd.log");
        let mut writer = RotatingFile::open(&path, 1).expect("open log");

        writer.write_all(b"aa").expect("first write");
        writer.write_all(b"bb").expect("second write");

        let rotated = rotated_logs(directory.path());
        assert_eq!(rotated.len(), 2);
        assert_ne!(rotated[0], rotated[1]);
        assert!(fs::read(&path).expect("read new log").is_empty());
    }
}
