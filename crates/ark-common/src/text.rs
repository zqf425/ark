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
    // Never reveal the whole value. A short secret used to be emitted
    // verbatim by `take(4)`; keep at most a quarter of the characters at each
    // end so the visible prefix and fingerprint tail can never reconstruct it.
    let chars: Vec<char> = trimmed.chars().collect();
    let shown = (chars.len() / 4).min(4);
    let visible: String = chars.iter().take(shown).collect();
    format!(
        "{visible}...{}",
        truncated_fingerprint(&chars, shown, value.len())
    )
}

fn truncated_fingerprint(chars: &[char], shown: usize, byte_len: usize) -> String {
    // Cheap, non-cryptographic fingerprint: stable length + a short tail.
    let tail: String = chars
        .iter()
        .rev()
        .take(shown)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{tail}({byte_len})")
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

    #[test]
    fn redaction_never_reveals_a_short_secret_in_full() {
        for secret in ["a", "abcd", "abcde", "12345678"] {
            let out = redact_secret(secret);
            assert!(!out.contains(secret), "{secret:?} leaked in full: {out:?}");
        }
        // Even the visible prefix and tail together cannot cover a short value.
        assert_eq!(redact_secret("a"), "...(1)");
    }
}
