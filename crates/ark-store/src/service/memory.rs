use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use ark_common::{read_json_file_opt, write_json_file, ArkError, Result};
use ark_contract::{
    CreateMemoryRequest, MemoryDiagnosticsResponse, MemoryEntry, MemoryListResponse,
    UpdateMemoryRequest,
};
use ark_ports::MemoryService;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum accepted UTF-8 byte length for one memory text.
pub const MAX_MEMORY_TEXT_BYTES: usize = 16 * 1024;

#[derive(Debug, Default, Serialize, Deserialize)]
struct MemoryFile {
    #[serde(default)]
    entries: Vec<MemoryEntry>,
    #[serde(default)]
    last_written_at: Option<DateTime<Utc>>,
}

/// JSON-backed implementation of the process-wide long-term memory store.
pub struct MemoryFileStore {
    path: PathBuf,
    state: Mutex<MemoryFile>,
}

impl MemoryFileStore {
    /// Open `<data_dir>/memory.json`; absent or damaged content starts empty.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join("memory.json");
        let state = read_json_file_opt(&path).unwrap_or_default();
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    fn lock(&self) -> MutexGuard<'_, MemoryFile> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn persist(&self, next: &MemoryFile) -> Result<()> {
        write_json_file(&self.path, next)
    }

    fn validate_text(text: &str) -> Result<()> {
        if text.trim().is_empty() {
            return Err(ArkError::validation("memory text must not be empty"));
        }
        if text.len() > MAX_MEMORY_TEXT_BYTES {
            return Err(ArkError::validation(format!(
                "memory text must not exceed {MAX_MEMORY_TEXT_BYTES} bytes"
            )));
        }
        Ok(())
    }

    fn write_state(&self, state: &mut MemoryFile) -> Result<()> {
        state.last_written_at = Some(Utc::now());
        self.persist(state)
    }
}

impl MemoryService for MemoryFileStore {
    fn list(&self) -> Result<MemoryListResponse> {
        Ok(MemoryListResponse {
            entries: self.lock().entries.clone(),
        })
    }

    fn create(&self, request: CreateMemoryRequest) -> Result<MemoryEntry> {
        Self::validate_text(&request.text)?;
        let now = Utc::now();
        let entry = MemoryEntry {
            id: format!("mem_{}", uuid::Uuid::new_v4()),
            text: request.text,
            tags: request.tags,
            source: request.source,
            created_at: now,
            updated_at: now,
        };
        let mut state = self.lock();
        let mut next = MemoryFile {
            entries: state.entries.clone(),
            last_written_at: state.last_written_at,
        };
        next.entries.push(entry.clone());
        self.write_state(&mut next)?;
        *state = next;
        Ok(entry)
    }

    fn update(&self, id: &str, request: UpdateMemoryRequest) -> Result<MemoryEntry> {
        if let Some(text) = request.text.as_deref() {
            Self::validate_text(text)?;
        }
        let mut state = self.lock();
        let mut next = MemoryFile {
            entries: state.entries.clone(),
            last_written_at: state.last_written_at,
        };
        let entry = next
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or_else(|| ArkError::not_found(format!("memory entry not found: {id}")))?;
        if let Some(text) = request.text {
            entry.text = text;
        }
        if let Some(tags) = request.tags {
            entry.tags = tags;
        }
        if request.source.is_some() {
            entry.source = request.source;
        }
        entry.updated_at = Utc::now();
        let updated = entry.clone();
        self.write_state(&mut next)?;
        *state = next;
        Ok(updated)
    }

    fn delete(&self, id: &str) -> Result<()> {
        let mut state = self.lock();
        let mut next = MemoryFile {
            entries: state.entries.clone(),
            last_written_at: state.last_written_at,
        };
        let index = next
            .entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or_else(|| ArkError::not_found(format!("memory entry not found: {id}")))?;
        next.entries.remove(index);
        self.write_state(&mut next)?;
        *state = next;
        Ok(())
    }

    fn diagnostics(&self) -> Result<MemoryDiagnosticsResponse> {
        let state = self.lock();
        Ok(MemoryDiagnosticsResponse {
            storage_path: self.path.to_string_lossy().to_string(),
            entry_count: state.entries.len() as u64,
            last_written_at: state.last_written_at,
        })
    }
}
