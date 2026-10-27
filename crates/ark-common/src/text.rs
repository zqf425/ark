//! UTF-8 safe truncation used at logging and display boundaries.

/// Truncate to at most `max_bytes` bytes **without** splitting a codepoint.
///
/// Returns the input unchanged when it already fits.
pub fn truncate_utf8(input: &str, max_bytes: usize) -> &str {
    if input.len() <= max_bytes {
        return input;
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

/// Redact obvious secrets for logs and event payloads.
///
/// Design §6.1 keeps redaction on the Rust side (decision R1) precisely so the
/// rules do not get re-implemented per client.
pub fn redact_secret(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let visible: String = trimmed.chars().take(4).collect();
    format!("{visible}...{}", truncated_fingerprint(trimmed))
}

fn truncated_fingerprint(value: &str) -> String {
    // Cheap, non-cryptographic fingerprint: stable length + tail.
    let tail: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}({})", tail, value.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "你好世界";
        assert_eq!(truncate_utf8(s, 4), "你");
        assert_eq!(truncate_utf8(s, 6), "你好");
        assert_eq!(truncate_utf8("abc", 10), "abc");
    }

    #[test]
    fn redaction_hides_the_body() {
        let out = redact_secret("sk-1234567890abcdef");
        assert!(!out.contains("567890"));
        assert!(out.starts_with("sk-1"));
    }
}
