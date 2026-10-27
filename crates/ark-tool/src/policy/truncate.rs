//! Output truncation policy (design §7.3).
//!
//! A tool that streams a 500 MB file into the model context is a runtime bug,
//! not a user mistake. Limits are data and applied in one place so `read`,
//! `grep`, `find` and `bash` all cut the same way and say the same thing.

/// Hard caps applied to tool output before it reaches the model context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TruncationLimits {
    /// Maximum bytes of tool content returned to the model.
    pub max_bytes: usize,
    /// Maximum lines returned for list-like tools.
    pub max_lines: usize,
    /// Maximum bytes of `bash` stdout+stderr kept on disk for `detail`.
    pub max_bash_bytes: usize,
    /// Default `bash` timeout when the call does not set one.
    pub default_timeout_ms: u64,
    /// Upper bound on a caller-supplied `bash` timeout.
    pub max_timeout_ms: u64,
}

/// Defaults chosen to keep one tool result well under a percent of context.
pub const DEFAULT_LIMITS: TruncationLimits = TruncationLimits {
    max_bytes: 256 * 1024,
    max_lines: 2_000,
    max_bash_bytes: 64 * 1024,
    default_timeout_ms: 120_000,
    max_timeout_ms: 600_000,
};

impl Default for TruncationLimits {
    fn default() -> Self {
        DEFAULT_LIMITS
    }
}

/// Cut `text` to at most `max` bytes on a character boundary, appending a marker.
pub fn cap_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[truncated: {} of {} bytes kept]",
        &text[..end],
        end,
        text.len()
    )
}

/// Keep the first `max` lines, appending a marker when lines were dropped.
pub fn cap_lines(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut kept = 0usize;
    let mut total = 0usize;
    for line in text.lines() {
        total += 1;
        if kept < max {
            out.push_str(line);
            out.push('\n');
            kept += 1;
        }
    }
    if total > max {
        out.push_str(&format!("[truncated: {kept} of {total} lines kept]\n"));
    }
    out
}

/// Apply both caps in the order that never splits a multi-byte character.
pub fn cap_all(text: &str, limits: &TruncationLimits, line_cap: Option<usize>) -> String {
    let lines_capped = match line_cap {
        Some(cap) => cap_lines(text, cap.min(limits.max_lines)),
        None => text.to_string(),
    };
    cap_bytes(&lines_capped, limits.max_bytes)
}

/// Clamp a requested timeout into `[1, max]`, falling back to the default.
pub fn clamp_timeout(requested: Option<u64>, limits: &TruncationLimits) -> u64 {
    match requested {
        Some(value) if value > 0 => value.min(limits.max_timeout_ms),
        _ => limits.default_timeout_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_untouched() {
        assert_eq!(cap_bytes("hello", 100), "hello");
    }

    #[test]
    fn byte_cap_marks_the_cut() {
        let capped = cap_bytes("abcdefghij", 5);
        assert!(capped.starts_with("abcde"));
        assert!(capped.contains("truncated"));
    }

    #[test]
    fn byte_cap_respects_char_boundaries() {
        let capped = cap_bytes("你好世界", 5);
        assert!(capped.starts_with("你"));
        assert!(capped.contains("truncated"));
    }

    #[test]
    fn line_cap_keeps_the_first_lines() {
        let capped = cap_lines("a\nb\nc\nd\n", 2);
        assert!(capped.starts_with("a\nb\n"));
        assert!(capped.contains("2 of 4 lines"));
    }

    #[test]
    fn line_cap_of_zero_yields_nothing() {
        assert_eq!(cap_lines("a\nb\n", 0), "");
    }

    #[test]
    fn timeout_falls_back_and_clamps() {
        let limits = TruncationLimits::default();
        assert_eq!(clamp_timeout(None, &limits), limits.default_timeout_ms);
        assert_eq!(clamp_timeout(Some(0), &limits), limits.default_timeout_ms);
        assert_eq!(clamp_timeout(Some(5), &limits), 5);
        assert_eq!(
            clamp_timeout(Some(u64::MAX), &limits),
            limits.max_timeout_ms
        );
    }

    #[test]
    fn cap_all_applies_the_narrower_line_cap() {
        let limits = TruncationLimits::default();
        let capped = cap_all("a\nb\nc\n", &limits, Some(1));
        assert!(capped.starts_with("a\n"));
        assert!(capped.contains("1 of 3 lines"));
    }
}
