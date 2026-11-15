//! Project-list projection for `GET /v1/workspace`.
//!
//! The "projects" a user can pick from are every workspace the system already
//! knows, not just the ones a global switch touched. Two sources feed the view:
//!
//! * the thread index — a thread's workspace override is persisted in
//!   `thread.json` and mirrored into the index, and it is what the session
//!   rail groups by. Any thread that was ever switched (session-level or
//!   `default_workspace` seeding) therefore contributes its root here.
//! * the process recent history — `WorkspaceRoots`' bounded recents written by
//!   the global `switch` surface (settings page).
//!
//! Merging both into one list keeps the composer's "existing projects" menu
//! and the session rail's project groups derived from the same fact: ark's
//! authority lives in the store, so the merge happens here, at the read
//! boundary.
//!
//! Paths are compared after normalization only for dedupe and grouping: the
//! descriptor always carries the first-seen original string (the canonical
//! thread/index form wins because thread roots come first in the merge
//! order), and no filesystem probing happens — a stale path stays visible
//! until the thread that references it is deleted, mirroring the rail.

use std::collections::HashMap;

use ark_contract::{WorkspaceDescriptor, WorkspaceView};

/// Upper bound on merged projects. Bounded so a long-lived installation
/// cannot grow the response without limit.
pub const MAX_WORKSPACE_PROJECTS: usize = 30;

/// One candidate source row. `active` is whether this workspace is the
/// runtime's active root (sorts first, like the sidebar).
pub struct ProjectCandidate {
    pub path: String,
    pub active: bool,
    /// Newest activity among the threads bound to this workspace; drives the
    /// recency ordering that the session rail already shows.
    pub last_event_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Normalized identity key for dedupe: case-insensitive, separator-insensitive,
/// without a trailing slash.
fn identity_key(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

/// Merge thread-index and recent-history workspaces into one project list.
///
/// Ordering (mirrors the sidebar): the active root first, then by the
/// workspace's most recent thread activity (newest first, never-active
/// last in input order), deduped by identity key. The active root is always
/// present even when no thread references it.
pub fn merge_workspace_projects(
    view: WorkspaceView,
    threads: impl IntoIterator<Item = ProjectCandidate>,
) -> WorkspaceView {
    let active_path = view
        .active
        .as_ref()
        .map(|descriptor| descriptor.path.clone())
        .unwrap_or_default();

    // First-seen-wins dedupe. Thread roots are inserted before recent roots so
    // the canonical stored form (index) is preferred over the history cache.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut merged: Vec<ProjectCandidate> = Vec::new();

    let push = |candidate: ProjectCandidate,
                seen: &mut HashMap<String, usize>,
                merged: &mut Vec<ProjectCandidate>| {
        let key = identity_key(&candidate.path);
        if key.is_empty() {
            return;
        }
        if let Some(&index) = seen.get(&key) {
            let existing = &mut merged[index];
            existing.active |= candidate.active;
            existing.last_event_at = existing.last_event_at.max(candidate.last_event_at);
            return;
        }
        seen.insert(key, merged.len());
        merged.push(candidate);
    };

    // The active root participates even without threads or recent entries, so
    // the composer always offers "where am I now".
    if !active_path.is_empty() {
        push(
            ProjectCandidate {
                path: active_path.clone(),
                active: true,
                last_event_at: None,
            },
            &mut seen,
            &mut merged,
        );
    }
    for candidate in threads {
        push(candidate, &mut seen, &mut merged);
    }
    for descriptor in view.recent {
        push(
            ProjectCandidate {
                path: descriptor.path,
                active: false,
                last_event_at: None,
            },
            &mut seen,
            &mut merged,
        );
    }

    // Active first, then newest thread activity, then input order (the merge
    // order above is already stable per source).
    merged.sort_by(|left, right| {
        right
            .active
            .cmp(&left.active)
            .then_with(|| right.last_event_at.cmp(&left.last_event_at))
    });
    merged.truncate(MAX_WORKSPACE_PROJECTS);

    WorkspaceView {
        generation: view.generation,
        // `active` is untouched: switching identity stays with `WorkspaceRoots`.
        active: view.active,
        recent: merged
            .into_iter()
            .filter(|candidate| {
                // The active root is reported through `active` itself, so it
                // does not double up as a history row.
                !(candidate.active && candidate.path == active_path)
            })
            .map(|candidate| WorkspaceDescriptor {
                path: candidate.path,
                // History entries are display-only; the runtime generation is
                // carried by `active`, exactly like `WorkspaceRoots::view_state`.
                generation: view.generation,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(path: &str) -> WorkspaceDescriptor {
        WorkspaceDescriptor {
            path: path.to_string(),
            generation: 0,
        }
    }

    fn thread(path: &str, at: Option<chrono::DateTime<chrono::Utc>>) -> ProjectCandidate {
        ProjectCandidate {
            path: path.to_string(),
            active: false,
            last_event_at: at,
        }
    }

    #[test]
    fn thread_workspace_that_never_switched_globally_is_listed() {
        // The reported bug: a session bound to /code/ark never went through the
        // global switch, so the composer menu could not offer it.
        let view = merge_workspace_projects(
            WorkspaceView {
                active: Some(descriptor("/home/.arkd/workspace")),
                recent: Vec::new(),
                generation: 3,
            },
            [thread("/code/ark", None)],
        );

        let paths: Vec<&str> = view.recent.iter().map(|d| d.path.as_str()).collect();
        assert!(
            paths.contains(&"/code/ark"),
            "thread-bound root must be offered: {paths:?}"
        );
    }

    #[test]
    fn active_root_sorts_first_and_stays_in_active() {
        let view = merge_workspace_projects(
            WorkspaceView {
                active: Some(descriptor("/w/active")),
                recent: vec![descriptor("/w/old")],
                generation: 1,
            },
            [thread("/w/old", Some(chrono::Utc::now()))],
        );

        assert_eq!(
            view.active.as_ref().map(|d| d.path.as_str()),
            Some("/w/active")
        );
        assert_eq!(
            view.recent[0].path, "/w/old",
            "the other root follows by recency"
        );
    }

    #[test]
    fn duplicate_paths_dedupe_preferring_thread_form() {
        let view = merge_workspace_projects(
            WorkspaceView {
                active: None,
                recent: vec![descriptor("/Code/Ark/")],
                generation: 0,
            },
            [thread("/code/ark", None)],
        );

        assert_eq!(view.recent.len(), 1);
        assert_eq!(
            view.recent[0].path, "/code/ark",
            "the thread/index form wins"
        );
    }

    #[test]
    fn recency_orders_projects_newest_thread_first() {
        let early = chrono::Utc::now() - chrono::Duration::hours(2);
        let late = chrono::Utc::now();
        let view = merge_workspace_projects(
            WorkspaceView {
                active: None,
                recent: Vec::new(),
                generation: 0,
            },
            [thread("/w/qdc", Some(early)), thread("/w/ark", Some(late))],
        );

        let paths: Vec<&str> = view.recent.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, vec!["/w/ark", "/w/qdc"]);
    }

    #[test]
    fn empty_paths_are_dropped() {
        let view = merge_workspace_projects(WorkspaceView::default(), [thread("   ", None)]);
        assert!(view.recent.is_empty());
    }

    #[test]
    fn merge_is_bounded() {
        let threads = (0..(MAX_WORKSPACE_PROJECTS + 10))
            .map(|index| thread(&format!("/w/project-{index}"), None));
        let view = merge_workspace_projects(WorkspaceView::default(), threads);
        assert_eq!(view.recent.len(), MAX_WORKSPACE_PROJECTS);
    }
}
