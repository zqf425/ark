//! Minimal glob matching for `find` and `grep --glob` (design §7.3).
//!
//! Deliberately in-house: the two patterns the tools need (`*.rs`, `src/**/*.ts`)
//! do not justify a dependency, and a pure function is easier to test than a
//! crate's semantics. Supported syntax: `*` (any run without `/`), `**` (any run
//! including `/`), `?` (one character), and literal everything else.

/// Match `text` against `pattern` with the syntax described above.
pub fn matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.replace('\\', "/").chars().collect();
    let text: Vec<char> = text.replace('\\', "/").chars().collect();
    walk(&pattern, &text)
}

fn walk(pattern: &[char], text: &[char]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }
    match pattern[0] {
        '*' if pattern.get(1) == Some(&'*') => {
            // `**` may cross separators; also swallow a following `/` so that
            // `src/**/x` matches `src/x`.
            let mut rest = &pattern[2..];
            while rest.first() == Some(&'*') {
                rest = &rest[1..];
            }
            if rest.first() == Some(&'/') {
                if walk(rest, text) {
                    return true;
                }
                rest = &rest[1..];
            }
            (0..=text.len()).any(|skip| walk(rest, &text[skip..]))
        }
        '*' => {
            let rest = &pattern[1..];
            (0..=text.len())
                .take_while(|skip| *skip == 0 || text[skip - 1] != '/')
                .any(|skip| walk(rest, &text[skip..]))
        }
        '?' => !text.is_empty() && text[0] != '/' && walk(&pattern[1..], &text[1..]),
        other => !text.is_empty() && text[0] == other && walk(&pattern[1..], &text[1..]),
    }
}

/// Whether `pattern` should be matched against a bare file name rather than a
/// slash-separated relative path.
pub fn is_name_pattern(pattern: &str) -> bool {
    !pattern.contains('/') && !pattern.contains("**")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_does_not_cross_separators() {
        assert!(matches("*.rs", "main.rs"));
        assert!(!matches("*.rs", "src/main.rs"));
        assert!(matches("src/*.rs", "src/main.rs"));
        assert!(!matches("src/*.rs", "src/a/main.rs"));
    }

    #[test]
    fn double_star_crosses_separators() {
        assert!(matches("src/**/*.rs", "src/a/b/main.rs"));
        assert!(matches("src/**/*.rs", "src/main.rs"));
        assert!(matches("**/*.md", "docs/a/b.md"));
    }

    #[test]
    fn question_mark_matches_one_character() {
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "ac"));
        assert!(!matches("a?c", "a/c"));
    }

    #[test]
    fn literal_patterns_match_exactly() {
        assert!(matches("Cargo.toml", "Cargo.toml"));
        assert!(!matches("Cargo.toml", "Cargo.lock"));
    }

    #[test]
    fn name_patterns_are_detected() {
        assert!(is_name_pattern("*.rs"));
        assert!(!is_name_pattern("src/*.rs"));
        assert!(!is_name_pattern("**/*.rs"));
    }

    #[test]
    fn backslashes_are_normalized() {
        assert!(matches("src/*.rs", "src\\main.rs"));
    }
}
