//! Context-window budget arithmetic shared by runtime and manual compaction.

/// Compaction trigger for a context window after reserving reply tokens.
pub fn compaction_threshold(context_window_tokens: u64, max_output_tokens: u64) -> u64 {
    let usable = context_window_tokens
        .saturating_sub(max_output_tokens)
        .max(1);
    (usable / 5 * 4 + usable % 5 * 4 / 5).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserves_output_before_applying_the_trigger_ratio() {
        assert_eq!(compaction_threshold(256_000, 16_000), 192_000);
        assert_eq!(compaction_threshold(1, 2), 1);
    }
}
