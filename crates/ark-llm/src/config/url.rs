//! URL construction for the three endpoint families.
//!
//! Mirrors the TypeScript runtime's `openai-compat-url.ts` behaviour so a
//! differential test can compare the two: trailing slashes are trimmed, a
//! version segment is appended only when the base does not already end in one,
//! and a `beta` base is promoted to its versioned sibling. Path appending is
//! kept strictly separate from JSON body fields.

/// Split a URL into (path, suffix) where `suffix` is the `?query` / `#fragment`
/// tail. Appending onto the path must never land after the query string.
fn split_suffix(url: &str) -> (&str, &str) {
    match url.find(['?', '#']) {
        Some(index) => (&url[..index], &url[index..]),
        None => (url, ""),
    }
}

fn trim_end(path: &str) -> &str {
    path.trim_end_matches('/')
}

fn append_path(base_url: &str, path: &str) -> String {
    let (path_part, suffix) = split_suffix(base_url);
    format!("{}/{}{}", trim_end(path_part), path, suffix)
}

fn last_segment(url: &str) -> &str {
    let (path, _) = split_suffix(url.trim());
    trim_end(path).rsplit('/').next().unwrap_or("")
}

fn is_version_segment(segment: &str) -> bool {
    let lower = segment.to_ascii_lowercase();
    if lower == "beta" {
        return true;
    }
    let Some(rest) = lower.strip_prefix('v') else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit())
}

fn unversioned(url: &str) -> String {
    let (path, suffix) = split_suffix(url);
    let trimmed = trim_end(path);
    match trimmed.rsplit_once('/') {
        Some((head, segment)) if is_version_segment(segment) => format!("{head}{suffix}"),
        _ => format!("{trimmed}{suffix}"),
    }
}

fn versioned(url: &str) -> String {
    let (path, suffix) = split_suffix(url.trim());
    let trimmed = format!("{}{}", trim_end(path), suffix);
    if is_version_segment(last_segment(&trimmed)) {
        return trimmed;
    }
    append_path(&trimmed, "v1")
}

/// `{base}/chat/completions`, promoting a `/beta` base to `/v1`.
pub fn openai_chat_url(base_url: &str) -> String {
    let mut base = versioned(base_url);
    if last_segment(&base).eq_ignore_ascii_case("beta") {
        base = append_path(&unversioned(base_url.trim()), "v1");
    }
    append_path(&base, "chat/completions")
}

/// `{base}/responses`, promoting a `/beta` base to `/v1`.
pub fn openai_responses_url(base_url: &str) -> String {
    let mut base = versioned(base_url);
    if last_segment(&base).eq_ignore_ascii_case("beta") {
        base = append_path(&unversioned(base_url.trim()), "v1");
    }
    append_path(&base, "responses")
}

/// `{base}/messages` for the Anthropic family, which uses `/v1` too.
pub fn anthropic_messages_url(base_url: &str) -> String {
    append_path(&versioned(base_url.trim()), "messages")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_host_gets_v1_and_the_path() {
        assert_eq!(
            openai_chat_url("https://api.deepseek.com"),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn existing_version_is_not_duplicated() {
        assert_eq!(
            openai_chat_url("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            openai_chat_url("https://api.deepseek.com/v1/"),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn beta_base_is_promoted_to_v1() {
        assert_eq!(
            openai_chat_url("https://api.deepseek.com/beta"),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn custom_prefix_is_preserved() {
        assert_eq!(
            openai_chat_url("https://gateway.example.com/openai"),
            "https://gateway.example.com/openai/v1/chat/completions"
        );
    }

    #[test]
    fn responses_url_uses_the_same_versioning() {
        assert_eq!(
            openai_responses_url("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1/responses"
        );
    }

    #[test]
    fn anthropic_url_appends_messages() {
        assert_eq!(
            anthropic_messages_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            anthropic_messages_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn query_string_stays_at_the_end() {
        assert_eq!(
            openai_chat_url("https://example.com/v1?key=1"),
            "https://example.com/v1/chat/completions?key=1"
        );
    }

    #[test]
    fn version_segment_detection_is_strict() {
        assert!(is_version_segment("v1"));
        assert!(is_version_segment("V2"));
        assert!(is_version_segment("beta"));
        assert!(!is_version_segment("v"));
        assert!(!is_version_segment("version"));
        assert!(!is_version_segment("v1beta"));
    }
}
