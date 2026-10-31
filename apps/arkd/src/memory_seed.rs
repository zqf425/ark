//! Prompt injection of the user-maintained long-term memory.
//!
//! This lives in the composition root on purpose. `ark-store` only reads and
//! writes the document, and `ark-agent` must stay unaware that a memory file
//! exists at all, so the single place that turns entries into a system message
//! is here. Everything in this module is pure: the caller does the file read, so
//! the budget and layout rules are unit-testable without touching a disk.

use ark_contract::MemoryEntry;
use ark_ports::ModelMessage;
use ark_store::MemoryFileStore;

/// Character budget for the bullet body (approved design, memory-store §3).
///
/// Counted in `char`s, not bytes: the store is UTF-8 and a byte budget would
/// silently halve the room available to a CJK user.
pub const MEMORY_SEED_BUDGET_CHARS: usize = 4096;

/// First line of the injected system message.
pub const MEMORY_SEED_HEADER: &str = "[user memory]";

/// Continuation indent that lines the wrapped text up behind `- `.
const BULLET_INDENT: &str = "  ";

/// Render the at-most-one `[user memory]` system message.
///
/// Entries are ordered oldest first so the same store always produces the same
/// bytes. Over budget the *newest* entries are dropped whole rather than
/// trimmed: half a memory reads as a complete statement, which is worse than
/// telling the model that older entries are all it gets.
///
/// Returns `None` for an empty store, and also when not even the oldest entry
/// fits - a header with no entry behind it is pure noise.
pub fn memory_seed_message(entries: &[MemoryEntry]) -> Option<ModelMessage> {
    let mut ordered: Vec<&MemoryEntry> = entries
        .iter()
        .filter(|entry| !entry.text.trim().is_empty())
        .collect();
    ordered.sort_by_key(|entry| entry.created_at);
    let total = ordered.len();
    let bullets: Vec<String> = ordered
        .iter()
        .map(|entry| bullet(entry.text.trim()))
        .collect();

    let mut shown = bullets.len();
    while shown > 0 && body_chars(&bullets[..shown]) > MEMORY_SEED_BUDGET_CHARS {
        shown -= 1;
    }
    if shown == 0 {
        return None;
    }

    let mut body = bullets[..shown].join("\n");
    if shown < total {
        body.push_str(&format!(
            "\n[user memory truncated: {shown} of {total} entries shown]"
        ));
    }
    Some(ModelMessage::system(format!(
        "{MEMORY_SEED_HEADER}\n{body}"
    )))
}

/// Memory for one turn, re-read from disk.
///
/// Memory is an enhancement, not a dependency: an unreadable or damaged
/// `memory.json` skips injection and warns instead of failing the turn. There is
/// deliberately no process-wide cache - the document holds hand-written entries,
/// so a plain re-read is what makes a memory the user just saved visible to the
/// very next turn.
pub fn seed_for_turn(memory: &MemoryFileStore) -> Option<ModelMessage> {
    match memory.read_entries() {
        Ok(entries) => memory_seed_message(&entries),
        Err(error) => {
            tracing::warn!(%error, "skipping user memory injection: memory.json is unreadable");
            None
        }
    }
}

/// One `- ` bullet whose wrapped lines keep the entry text verbatim.
fn bullet(text: &str) -> String {
    let mut out = String::from("- ");
    for (index, line) in text.lines().enumerate() {
        if index != 0 {
            out.push('\n');
            out.push_str(BULLET_INDENT);
        }
        out.push_str(line);
    }
    out
}

/// Character count of the joined bullets, including their separators.
fn body_chars(bullets: &[String]) -> usize {
    let lines: usize = bullets.iter().map(|bullet| bullet.chars().count()).sum();
    lines + bullets.len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ports::MemoryService;
    use chrono::{TimeZone, Utc};

    /// Count entry bullets, ignoring the `[user memory]` header line.
    fn bullet_lines(content: &str) -> usize {
        content
            .lines()
            .filter(|line| line.starts_with("- "))
            .count()
    }

    fn entry(minute: u32, text: &str) -> MemoryEntry {
        let created = Utc.with_ymd_and_hms(2026, 11, 1, 9, minute, 0).unwrap();
        MemoryEntry {
            id: format!("mem_{minute}"),
            text: text.to_string(),
            tags: Vec::new(),
            source: None,
            created_at: created,
            updated_at: created,
        }
    }

    #[test]
    fn empty_store_injects_nothing() {
        assert!(memory_seed_message(&[]).is_none());
    }

    #[test]
    fn entries_are_ordered_oldest_first_in_one_system_message() {
        // Passed newest first to prove the order comes from `created_at`.
        let entries = vec![entry(2, "second"), entry(1, "first")];
        let message = memory_seed_message(&entries).expect("two entries fit");
        assert_eq!(message.role, ark_ports::ModelRole::System);
        assert_eq!(message.content, "[user memory]\n- first\n- second");
    }

    #[test]
    fn multiline_entries_keep_their_lines_aligned_after_the_bullet() {
        let entries = vec![entry(1, "line one\nline two")];
        let message = memory_seed_message(&entries).expect("one entry fits");
        assert_eq!(message.content, "[user memory]\n- line one\n  line two");
    }

    #[test]
    fn over_budget_drops_whole_newest_entries_and_annotates_the_count() {
        // Two bullets of 2002 chars fit in 4096; the third cannot.
        let entries = vec![
            entry(1, &format!("oldest {}", "a".repeat(2000))),
            entry(2, &format!("middle {}", "b".repeat(2000))),
            entry(3, &format!("newest {}", "c".repeat(2000))),
        ];
        let message = memory_seed_message(&entries).expect("two entries fit");
        assert!(message.content.starts_with("[user memory]\n- oldest "));
        assert!(message.content.contains("\n- middle "));
        assert!(
            !message.content.contains("newest"),
            "the dropped entry must not survive as a fragment"
        );
        assert!(message
            .content
            .ends_with("[user memory truncated: 2 of 3 entries shown]"));
        // Exactly two bullets, each complete.
        assert_eq!(bullet_lines(&message.content), 2);
        assert!(message.content.contains(&"b".repeat(2000)));
    }

    #[test]
    fn budget_counts_characters_not_bytes() {
        // 1500 CJK chars are 4500 bytes but only 1500 characters, so two such
        // entries must both survive a 4096-character budget.
        let wide = "\u{4e2d}".repeat(1500);
        let entries = vec![entry(1, &wide), entry(2, &wide)];
        let message = memory_seed_message(&entries).expect("char budget fits");
        assert!(!message.content.contains("truncated"));
        assert_eq!(bullet_lines(&message.content), 2);
    }

    #[test]
    fn an_entry_that_alone_exceeds_the_budget_injects_nothing() {
        let entries = vec![entry(1, &"x".repeat(MEMORY_SEED_BUDGET_CHARS + 1))];
        assert!(memory_seed_message(&entries).is_none());
    }

    /// Temp data dir that removes itself, so the disk-path tests leave nothing.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time is after the Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("ark-memory-seed-{nonce}"));
            std::fs::create_dir_all(&path).expect("temp data dir created");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_document_injects_nothing() {
        let dir = TempDir::new();
        let store = MemoryFileStore::open(&dir.0).expect("store opens");
        assert!(seed_for_turn(&store).is_none());
    }

    #[test]
    fn a_damaged_document_skips_injection_instead_of_failing() {
        let dir = TempDir::new();
        let store = MemoryFileStore::open(&dir.0).expect("store opens");
        std::fs::write(dir.0.join("memory.json"), b"not json").expect("damaged file written");
        // The turn must survive: no message, no error, no panic.
        assert!(seed_for_turn(&store).is_none());
    }

    #[test]
    fn a_document_written_after_open_is_visible_to_the_next_turn() {
        let dir = TempDir::new();
        let store = MemoryFileStore::open(&dir.0).expect("store opens");
        assert!(seed_for_turn(&store).is_none(), "starts empty");

        // Written through the store, then read back by a separate turn call.
        store
            .create(ark_contract::CreateMemoryRequest {
                text: "prefers short answers".to_string(),
                tags: Vec::new(),
                source: None,
            })
            .expect("create succeeds");
        let message = seed_for_turn(&store).expect("next turn sees the new memory");
        assert_eq!(message.content, "[user memory]\n- prefers short answers");
    }
}
