//! Process-owned active workspace state.
//!
//! A turn receives a copy of [`WorkspaceLease`] before it starts. It never
//! consults configuration again, so a switch cannot make one model turn run
//! tools in two different roots.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use ark_common::{read_json_file_opt, write_json_file, ArkError, Result};
use ark_contract::{WorkspaceDescriptor, WorkspaceView};
use serde::{Deserialize, Serialize};

const RECENT_LIMIT: usize = 10;
const RECENT_FILE: &str = "workspace-recent.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct RecentDocument {
    #[serde(default)]
    recent: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLease {
    pub canonical_root: PathBuf,
    pub generation: u64,
}

#[derive(Debug)]
struct State {
    active: WorkspaceLease,
    recent: Vec<PathBuf>,
}

/// Serializes workspace changes and owns the active canonical root.
pub struct WorkspaceRoots {
    recent_path: PathBuf,
    recent_write: Mutex<()>,
    state: Mutex<State>,
}

impl WorkspaceRoots {
    /// Creates the runtime state from the CLI fallback once at process start.
    /// Missing or damaged recent data is a cache miss and never blocks startup.
    pub fn new(initial: PathBuf, data_dir: &Path) -> Result<Self> {
        let canonical_root = canonical_directory(&initial)?;
        let recent_path = data_dir.join(RECENT_FILE);
        let recent = load_recent(&recent_path, &canonical_root);
        Ok(Self {
            recent_path,
            recent_write: Mutex::new(()),
            state: Mutex::new(State {
                active: WorkspaceLease {
                    canonical_root,
                    generation: 0,
                },
                recent,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Captures the root for one parent or child turn.
    pub fn lease(&self) -> WorkspaceLease {
        self.lock().active.clone()
    }

    /// Compatibility helper for components that only need a path outside a turn.
    pub fn current(&self) -> PathBuf {
        self.lease().canonical_root
    }

    pub fn view(&self) -> WorkspaceView {
        view_state(&self.lock())
    }

    /// Changes roots only while no parent or child is active. The caller passes
    /// its authoritative activity count while holding the server's workspace
    /// gate, so a switch never cancels work implicitly.
    pub fn switch(&self, raw: &str, active: usize) -> Result<WorkspaceView> {
        if active != 0 {
            return Err(ArkError::conflict(format!(
                "cannot switch workspace while {active} turn(s) are active"
            )));
        }
        let canonical_root = canonical_directory(Path::new(raw))?;
        // This lock serializes writers after the state lock is released. It does
        // not block lease readers while the small derived cache is fsynced.
        let _write_guard = self
            .recent_write
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (view, recent) = {
            let mut state = self.lock();
            if state.active.canonical_root == canonical_root {
                return Ok(view_state(&state));
            }

            let old = state.active.canonical_root.clone();
            state
                .recent
                .retain(|path| path != &canonical_root && path != &old);
            state.recent.insert(0, old);
            state.recent.truncate(RECENT_LIMIT);
            state.active = WorkspaceLease {
                canonical_root,
                generation: state.active.generation.saturating_add(1),
            };
            (view_state(&state), state.recent.clone())
        };

        // Recents are display-only derived data. A full/read-only data-dir must
        // not turn an otherwise valid workspace switch into an authorization or
        // availability failure; the next successful switch can rebuild it.
        if let Err(error) = write_json_file(&self.recent_path, &RecentDocument { recent }) {
            tracing::warn!(
                path = %self.recent_path.display(),
                error = %error,
                "could not persist recent workspaces"
            );
        }
        Ok(view)
    }
}

fn load_recent(path: &Path, active: &Path) -> Vec<PathBuf> {
    let document: RecentDocument = read_json_file_opt(path).unwrap_or_default();
    let mut recent = Vec::new();
    for stored in document.recent {
        // Recents are display-only derived data, never an authorization root.
        // Re-canonicalizing also drops deleted or inaccessible directories so
        // stale entries do not survive indefinitely after a restart.
        let Ok(canonical) = canonical_directory(&stored) else {
            continue;
        };
        if canonical != active && !recent.contains(&canonical) {
            recent.push(canonical);
        }
        if recent.len() == RECENT_LIMIT {
            break;
        }
    }
    recent
}

fn view_state(state: &State) -> WorkspaceView {
    WorkspaceView {
        active: Some(descriptor(&state.active)),
        recent: state
            .recent
            .iter()
            .map(|path| WorkspaceDescriptor {
                path: path.display().to_string(),
                generation: state.active.generation,
            })
            .collect(),
        generation: state.active.generation,
    }
}

fn descriptor(lease: &WorkspaceLease) -> WorkspaceDescriptor {
    WorkspaceDescriptor {
        path: lease.canonical_root.display().to_string(),
        generation: lease.generation,
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf> {
    if !path.is_dir() {
        return Err(ArkError::validation(format!(
            "workspace path is not a directory: {}",
            path.display()
        )));
    }
    path.canonicalize()
        .map(|path| PathBuf::from(ark_common::path::strip_verbatim(&path.to_string_lossy())))
        .map_err(|_| {
            ArkError::validation(format!("cannot canonicalize workspace: {}", path.display()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "arkd-workspace-{label}-{}",
                ark_common::TurnId::new()
            ));
            std::fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn child(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir_all(&path).expect("create workspace directory");
            path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn lease_keeps_the_old_root_after_a_switch() {
        let test = TestDir::new("lease");
        let first = test.child("first");
        let second = test.child("second");
        let roots = WorkspaceRoots::new(first, &test.0).expect("initial root");
        let lease = roots.lease();
        roots
            .switch(&second.display().to_string(), 0)
            .expect("switch");
        assert_ne!(lease.canonical_root, roots.current());
        assert_eq!(lease.generation, 0);
        assert_eq!(roots.lease().generation, 1);
    }

    #[test]
    fn active_turn_blocks_switch() {
        let test = TestDir::new("busy");
        let first = test.child("first");
        let second = test.child("second");
        let roots = WorkspaceRoots::new(first.clone(), &test.0).expect("initial root");
        let error = roots
            .switch(&second.display().to_string(), 2)
            .expect_err("busy");
        assert_eq!(error.code, ark_common::ArkErrorCode::Conflict);
        assert_eq!(
            roots.current(),
            canonical_directory(&first).expect("canonical")
        );
        assert_eq!(roots.lease().generation, 0);
        assert!(!test.0.join(RECENT_FILE).exists());
    }

    #[test]
    fn recent_workspaces_survive_restart_as_bom_free_canonical_json() {
        let test = TestDir::new("persist");
        let first = test.child("first");
        let second = test.child("second");
        let third = test.child("third");
        let roots = WorkspaceRoots::new(first.clone(), &test.0).expect("initial root");

        roots
            .switch(&second.display().to_string(), 0)
            .expect("first switch");
        let view = roots
            .switch(&third.display().to_string(), 0)
            .expect("second switch");
        assert_eq!(view.generation, 2);

        let path = test.0.join(RECENT_FILE);
        let bytes = std::fs::read(&path).expect("read recent file");
        assert!(!bytes.starts_with(&ark_common::UTF8_BOM_BYTES));
        std::str::from_utf8(&bytes).expect("recent file is UTF-8");

        let restarted = WorkspaceRoots::new(third, &test.0).expect("restart");
        let recent: Vec<_> = restarted
            .view()
            .recent
            .into_iter()
            .map(|entry| PathBuf::from(entry.path))
            .collect();
        assert_eq!(
            recent,
            vec![
                canonical_directory(&second).expect("canonical second"),
                canonical_directory(&first).expect("canonical first"),
            ]
        );
    }

    #[test]
    fn damaged_recent_file_degrades_to_empty() {
        let test = TestDir::new("damaged");
        let active = test.child("active");
        std::fs::write(test.0.join(RECENT_FILE), b"{not json").expect("damage cache");

        let roots = WorkspaceRoots::new(active, &test.0).expect("startup must continue");
        assert!(roots.view().recent.is_empty());
    }

    #[test]
    fn startup_drops_missing_non_directory_duplicate_and_active_recents() {
        let test = TestDir::new("stale");
        let active = test.child("active");
        let valid = test.child("valid");
        let missing = test.child("missing");
        let file = test.0.join("not-a-directory.txt");
        std::fs::remove_dir(&missing).expect("remove stale workspace");
        std::fs::write(&file, b"not a workspace").expect("create file entry");
        write_json_file(
            &test.0.join(RECENT_FILE),
            &RecentDocument {
                recent: vec![missing, file, valid.clone(), valid.clone(), active.clone()],
            },
        )
        .expect("seed cache");

        let roots = WorkspaceRoots::new(active, &test.0).expect("startup");
        let recent = roots.view().recent;
        assert_eq!(recent.len(), 1);
        assert_eq!(
            PathBuf::from(&recent[0].path),
            canonical_directory(&valid).expect("canonical valid")
        );
    }

    #[test]
    fn persistence_failure_does_not_block_switch_or_generation() {
        let test = TestDir::new("persist-failure");
        let first = test.child("first");
        let second = test.child("second");
        std::fs::create_dir(test.0.join(RECENT_FILE)).expect("block recent file rename");
        let roots = WorkspaceRoots::new(first.clone(), &test.0).expect("initial root");

        let view = roots
            .switch(&second.display().to_string(), 0)
            .expect("derived cache failure must not block switch");

        assert_eq!(view.generation, 1);
        assert_eq!(
            roots.current(),
            canonical_directory(&second).expect("canonical second")
        );
        assert_eq!(
            PathBuf::from(&view.recent[0].path),
            canonical_directory(&first).expect("canonical first")
        );
    }

    #[test]
    fn exposed_workspace_root_is_not_windows_verbatim() {
        let test = TestDir::new("display");
        let roots = WorkspaceRoots::new(test.child("active"), &test.0).expect("root");
        assert!(
            !roots.current().to_string_lossy().starts_with(r"\\?\"),
            "workspace root must be suitable for prompts and tool inputs"
        );
    }
}
