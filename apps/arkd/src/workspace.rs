//! Process-owned active workspace state.
//!
//! A turn receives a copy of [`WorkspaceLease`] before it starts. It never
//! consults configuration again, so a switch cannot make one model turn run
//! tools in two different roots.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use ark_common::{ArkError, Result};
use ark_contract::{WorkspaceDescriptor, WorkspaceView};

const RECENT_LIMIT: usize = 10;

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
    state: Mutex<State>,
}

impl WorkspaceRoots {
    /// Creates the runtime state from the CLI fallback once at process start.
    pub fn new(initial: PathBuf) -> Result<Self> {
        let canonical_root = canonical_directory(&initial)?;
        Ok(Self {
            state: Mutex::new(State {
                active: WorkspaceLease {
                    canonical_root,
                    generation: 0,
                },
                recent: Vec::new(),
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
        let state = self.lock();
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
        let mut state = self.lock();
        if state.active.canonical_root != canonical_root {
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
        }
        drop(state);
        Ok(self.view())
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
    path.canonicalize().map_err(|_| {
        ArkError::validation(format!("cannot canonicalize workspace: {}", path.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_keeps_the_old_root_after_a_switch() {
        let first = std::env::temp_dir();
        let second = std::env::current_dir().expect("cwd");
        let roots = WorkspaceRoots::new(first).expect("initial root");
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
        let root = std::env::temp_dir();
        let roots = WorkspaceRoots::new(root.clone()).expect("initial root");
        let error = roots
            .switch(&root.display().to_string(), 2)
            .expect_err("busy");
        assert_eq!(error.code, ark_common::ArkErrorCode::Conflict);
    }
}
