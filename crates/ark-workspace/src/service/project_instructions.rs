//! Project instruction files (`docs/requirements/project-instructions.md`) —
//! the single workspace discovery and truncation host.
//!
//! A repository's own instructions (`AGENTS.md` / `CLAUDE.md` / `README.md`)
//! are the first context a coding agent needs. They are **workspace facts**,
//! so discovery is anchored at the turn's effective workspace root: the same
//! boundary the sandbox authorizes tools against. The composition root only
//! appends the returned text while assembling the turn seed.

use std::path::Path;

/// Candidate file names in priority order. The first hit wins; `README.md` is
/// the deliberate fallback so repositories without an agent-specific file
/// still get their project description injected.
const CANDIDATES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "README.md"];

/// UTF-8 byte budget for the injected text. A giant README must not eat the
/// context budget; 32 KiB covers real instruction files (this repository's own
/// AGENTS.md is ~8 KiB).
const MAX_BYTES: usize = 32 * 1024;

/// Marker injected before the instruction text, so the model can tell the
/// repository's voice apart from ark's own system prompt.
pub const PROJECT_INSTRUCTIONS_HEADER: &str = "[project instructions]";

/// Truncated tail appended when the file exceeds the budget.
const TRUNCATED_TAIL: &str = "[project instructions truncated]";

/// Read the workspace's project instructions, already wrapped as a full
/// message body (`header\n\ntext`), or `None` when nothing usable exists.
///
/// A missing, unreadable, non-UTF-8 or blank file is "no instructions": the
/// turn proceeds and the failure is only a warning, never an error.
pub fn seed_for_workspace(workspace: &Path) -> Option<String> {
    let text = read_first_hit(workspace)?;
    let truncated = ark_common::truncate_utf8(&text, MAX_BYTES);
    if truncated.trim().is_empty() {
        return None;
    }
    let body = if truncated.len() < text.len() {
        format!("{truncated}\n{TRUNCATED_TAIL}")
    } else {
        truncated.to_string()
    };
    Some(format!("{PROJECT_INSTRUCTIONS_HEADER}\n\n{body}"))
}

/// Read the first candidate that exists, in priority order.
fn read_first_hit(workspace: &Path) -> Option<String> {
    for name in CANDIDATES {
        let path = workspace.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        match String::from_utf8(bytes) {
            Ok(text) => {
                if text.trim().is_empty() {
                    tracing::warn!(file = %path.display(), "project instructions file is empty; skipped");
                    return None;
                }
                return Some(text);
            }
            Err(_) => {
                tracing::warn!(file = %path.display(), "project instructions file is not UTF-8; skipped");
                return None;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ark-instructions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        dir
    }

    #[test]
    fn agents_md_wins_over_claude_and_readme() {
        let dir = fixture("priority");
        std::fs::write(dir.join("AGENTS.md"), "agents rules").unwrap();
        std::fs::write(dir.join("CLAUDE.md"), "claude rules").unwrap();
        std::fs::write(dir.join("README.md"), "readme").unwrap();
        let seed = seed_for_workspace(&dir).unwrap();
        assert!(seed.contains("agents rules"));
        assert!(!seed.contains("claude rules"));
    }

    #[test]
    fn claude_md_wins_over_readme() {
        let dir = fixture("claude-first");
        std::fs::write(dir.join("CLAUDE.md"), "claude rules").unwrap();
        std::fs::write(dir.join("README.md"), "readme").unwrap();
        let seed = seed_for_workspace(&dir).unwrap();
        assert!(seed.contains("claude rules"));
        assert!(!seed.contains("readme"));
    }

    #[test]
    fn readme_is_the_fallback() {
        let dir = fixture("readme-only");
        std::fs::write(dir.join("README.md"), "project description").unwrap();
        let seed = seed_for_workspace(&dir).unwrap();
        assert!(seed.contains("project description"));
    }

    #[test]
    fn no_candidate_yields_none() {
        let dir = fixture("empty-dir");
        assert!(seed_for_workspace(&dir).is_none());
    }

    #[test]
    fn blank_file_yields_none() {
        let dir = fixture("blank");
        std::fs::write(dir.join("AGENTS.md"), "   \n\t\n").unwrap();
        assert!(seed_for_workspace(&dir).is_none());
    }

    #[test]
    fn non_utf8_file_yields_none() {
        let dir = fixture("binary");
        std::fs::write(dir.join("AGENTS.md"), [0xff, 0xfe, 0x00, 0x01]).unwrap();
        assert!(seed_for_workspace(&dir).is_none());
    }

    #[test]
    fn oversized_file_is_truncated_on_a_char_boundary() {
        let dir = fixture("truncate");
        // A run of multi-byte characters: every char is 3 bytes, so a naive
        // byte cut would split one.
        let body = "你".repeat(20_000);
        std::fs::write(dir.join("AGENTS.md"), &body).unwrap();
        let seed = seed_for_workspace(&dir).unwrap();
        assert!(seed.contains(TRUNCATED_TAIL));
        assert!(seed.len() < body.len());
        // The injected text minus the tail must still be valid UTF-8 (it is a
        // `String` by construction) and must not end mid-character: dropping
        // the tail leaves whole characters only.
        let without_tail = seed.replace(TRUNCATED_TAIL, "");
        assert!(!without_tail.ends_with('\u{fffd}'));
    }

    #[test]
    fn seed_carries_the_header() {
        let dir = fixture("header");
        std::fs::write(dir.join("AGENTS.md"), "rule one").unwrap();
        let seed = seed_for_workspace(&dir).unwrap();
        assert!(seed.starts_with(PROJECT_INSTRUCTIONS_HEADER));
        assert!(seed.contains("rule one"));
    }
}
