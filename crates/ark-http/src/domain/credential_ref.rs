//! Credential reference syntax (design 7.5.1: name-only references).
//!
//! A plugin never receives a secret and never chooses the `Authorization`
//! header directly. It references a stored credential by name; the optional
//! `@<header>` suffix lets a provider that expects `x-api-key` still work
//! without handing header control to the plugin.

use ark_common::{ArkError, Result};

/// Well-known non-`Authorization` injection targets. Anything else is denied,
/// so a plugin cannot smuggle a reserved header through a credential name.
const ALLOWED_HEADERS: [&str; 6] = [
    "x-api-key",
    "api-key",
    "apikey",
    "private-token",
    "x-auth-token",
    "x-goog-api-key",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRef {
    /// Owner-local credential name, e.g. `token`.
    pub name: String,
    /// Header the bus will set. `authorization` means `Bearer <secret>`.
    pub header: String,
    pub bearer: bool,
}

impl CredentialRef {
    pub fn header_value(&self, secret: &str) -> String {
        if self.bearer {
            format!("Bearer {secret}")
        } else {
            secret.to_string()
        }
    }
}

/// Parse `name` or `name@header`.
pub fn parse_reference(raw: &str) -> Result<CredentialRef> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ArkError::validation("credential reference is empty"));
    }
    match raw.split_once('@') {
        None => Ok(CredentialRef {
            name: raw.to_string(),
            header: "authorization".to_string(),
            bearer: true,
        }),
        Some((name, header)) => {
            let name = name.trim();
            let header = header.trim().to_ascii_lowercase();
            if name.is_empty() {
                return Err(ArkError::validation("credential name is empty"));
            }
            if !ALLOWED_HEADERS.contains(&header.as_str()) {
                return Err(ArkError::validation(format!(
                    "credential target header {header:?} is not injectable"
                ))
                .with_detail("reserved_header"));
            }
            Ok(CredentialRef {
                name: name.to_string(),
                header,
                bearer: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_name_injects_bearer_authorization() {
        let cred = parse_reference("token").unwrap();
        assert_eq!(cred.header, "authorization");
        assert!(cred.bearer);
        assert_eq!(cred.header_value("s3cret"), "Bearer s3cret");
    }

    #[test]
    fn explicit_allowlisted_header_is_raw() {
        let cred = parse_reference("token@x-api-key").unwrap();
        assert_eq!(cred.header, "x-api-key");
        assert!(!cred.bearer);
        assert_eq!(cred.header_value("s3cret"), "s3cret");
    }

    #[test]
    fn reserved_headers_cannot_be_selected() {
        for bad in [
            "token@host",
            "token@user-agent",
            "token@x-plugin-id",
            "token@authorization",
        ] {
            assert!(parse_reference(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn empty_reference_is_rejected() {
        assert!(parse_reference("").is_err());
        assert!(parse_reference("@x-api-key").is_err());
    }
}
