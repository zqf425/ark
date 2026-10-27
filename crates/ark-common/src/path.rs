//! Canonical path normalization (design §6.1, decision R2).
//!
//! This is the single implementation. The renderer calls it through the typed
//! client / future WASM build; it must never re-implement it in TypeScript.

/// Normalize a path for display and comparison:
///
/// * backslashes become `/`
/// * duplicate separators collapse
/// * `.` segments are dropped, `..` pops the previous segment when possible
/// * a drive prefix (`C:`) and a leading `/` are preserved
///
/// The result is not a filesystem path; it is a stable comparison key.
pub fn normalize(input: &str) -> String {
    let unified = input.replace('\\', "/");

    let (prefix, rest) = split_prefix(&unified);
    let absolute = rest.starts_with('/');

    let mut stack: Vec<&str> = Vec::new();
    for segment in rest.split('/') {
        match segment {
            "" | "." => continue,
            ".." => {
                if matches!(stack.last(), Some(last) if *last != "..") {
                    stack.pop();
                } else if !absolute {
                    stack.push("..");
                }
            }
            other => stack.push(other),
        }
    }

    let joined = stack.join("/");
    match (prefix.is_empty(), absolute) {
        (true, true) => format!("/{joined}"),
        (true, false) => joined,
        (false, true) => format!("{prefix}/{joined}"),
        (false, false) => format!("{prefix}{joined}"),
    }
}

fn split_prefix(input: &str) -> (String, &str) {
    let bytes = input.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return (input[..2].to_string(), &input[2..]);
    }
    (String::new(), input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_separators_and_dot_segments() {
        assert_eq!(normalize("a//b/./c"), "a/b/c");
        assert_eq!(normalize("a\\b\\c"), "a/b/c");
    }

    #[test]
    fn resolves_parent_segments() {
        assert_eq!(normalize("/a/b/../c"), "/a/c");
        assert_eq!(normalize("a/../../b"), "../b");
    }

    #[test]
    fn keeps_windows_drive_prefix() {
        assert_eq!(normalize(r"C:\Users\x\..\y"), "C:/Users/y");
        assert_eq!(normalize("c:/a//b"), "c:/a/b");
    }
}
