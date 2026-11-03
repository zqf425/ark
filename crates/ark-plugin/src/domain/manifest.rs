//! Plugin manifest and version rules (design 7.7.1).

use ark_common::{ArkError, Result};
use ark_contract::Capability;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::domain::identity::validate_plugin_id;

/// What kind of contribution a plugin makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    Tool,
    ModelProvider,
    McpClient,
    StudioAction,
    /// Registers a page (the Code screen itself travels this path).
    UiPage,
    /// Registers only an embeddable panel, taking no navigation slot.
    UiPanel,
    Backend,
}

impl PluginKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PluginKind::Tool => "tool",
            PluginKind::ModelProvider => "model_provider",
            PluginKind::McpClient => "mcp_client",
            PluginKind::StudioAction => "studio_action",
            PluginKind::UiPage => "ui_page",
            PluginKind::UiPanel => "ui_panel",
            PluginKind::Backend => "backend",
        }
    }
}

/// Semantic version, minimal on purpose: only major/minor/patch plus optional
/// pre-release text, because that is all dependency resolution needs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre: Option<String>,
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: None,
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        let (core, pre) = match raw.split_once('-') {
            Some((core, pre)) => (core, Some(pre.to_string())),
            None => (raw, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() < 2 || parts.len() > 3 {
            return Err(ArkError::validation(format!("invalid version {raw:?}")));
        }
        let mut numbers = [0u64; 3];
        for (idx, part) in parts.iter().enumerate() {
            numbers[idx] = part
                .parse::<u64>()
                .map_err(|_| ArkError::validation(format!("invalid version {raw:?}")))?;
        }
        Ok(Self {
            major: numbers[0],
            minor: numbers[1],
            patch: numbers[2],
            pre,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.pre {
            Some(pre) => write!(f, "{}.{}.{}-{}", self.major, self.minor, self.patch, pre),
            None => write!(f, "{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

/// A version requirement. `^1` / `^1.2` / `^1.2.3` all mean "same major".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionReq {
    /// Original text, kept for diagnostics and lock manifests.
    pub raw: String,
    pub major: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor: Option<u64>,
}

impl VersionReq {
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(ArkError::validation("empty version requirement"));
        }
        let body = raw.strip_prefix('^').unwrap_or(raw);
        if body.starts_with('~')
            || body.starts_with('>')
            || body.starts_with('<')
            || body.contains('*')
        {
            return Err(ArkError::validation(format!(
                "unsupported version requirement {raw:?}; use ^major[.minor[.patch]]"
            )));
        }
        let parts: Vec<&str> = body.split('.').collect();
        if parts.is_empty() || parts.len() > 3 {
            return Err(ArkError::validation(format!(
                "invalid version requirement {raw:?}"
            )));
        }
        let major = parts[0]
            .parse::<u64>()
            .map_err(|_| ArkError::validation(format!("invalid version requirement {raw:?}")))?;
        let minor = match parts.get(1) {
            Some(value) => Some(value.parse::<u64>().map_err(|_| {
                ArkError::validation(format!("invalid version requirement {raw:?}"))
            })?),
            None => None,
        };
        Ok(Self {
            raw: raw.to_string(),
            major,
            minor,
        })
    }

    /// Major must match; `minor` in the requirement is advisory (compatible).
    pub fn accepts(&self, version: &Version) -> bool {
        if self.major != version.major {
            return false;
        }
        match self.minor {
            Some(minor) => version.minor >= minor,
            None => true,
        }
    }
}

/// `manifest.http` (design 7.5.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpSection {
    /// Host allowlist; empty means "no egress".
    #[serde(default)]
    pub allow: Vec<String>,
}

/// `manifest.mcp` (design 7.5.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSection {
    #[serde(default)]
    pub expose: Vec<String>,
    #[serde(default)]
    pub consume: Vec<String>,
}

/// `manifest.hooks` (requirements/plugin-hook-bus.md).
///
/// Declaring hooks is an authorization *request*, never a grant: the host turns
/// each declared point into a typed handle only for plugins it trusts (v1:
/// `ark.builtin.*`). A directory plugin has no code to receive a callback, so
/// the loader skips any directory plugin that ships this section (fail closed).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookSection {
    /// Hook ABI the plugin was written against; the host refuses a mismatch
    /// instead of partially honouring a payload shape it does not know.
    #[serde(default)]
    pub hook_abi: u32,
    /// Requested hook points, e.g. `["llm.request", "tool.pre_call"]`.
    #[serde(default)]
    pub points: Vec<String>,
}

/// The declaration a plugin ships.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub version: Version,
    pub kind: PluginKind,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// JSON Schema of the plugin's own config. Kept opaque here: validation is
    /// the config service's job, and this crate must not depend on a schema
    /// implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<serde_json::Value>,
    /// service/plugin id -> version requirement (`{"ark.http": "^1"}`).
    #[serde(default)]
    pub requires: BTreeMap<String, String>,
    #[serde(default)]
    pub http: HttpSection,
    #[serde(default)]
    pub mcp: McpSection,
    /// Hook authorization request. Absent on every manifest written before the
    /// hook bus existed, so old plugins parse exactly as they did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<HookSection>,
}

impl PluginManifest {
    /// Minimal valid manifest, mostly for tests and scaffolding.
    pub fn new(id: &str, version: &str, kind: PluginKind) -> Result<Self> {
        let manifest = Self {
            id: id.to_string(),
            version: Version::parse(version)?,
            kind,
            capabilities: Vec::new(),
            config_schema: None,
            requires: BTreeMap::new(),
            http: HttpSection::default(),
            mcp: McpSection::default(),
            hooks: None,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Structural validation. Called before any plugin code runs, so a bad
    /// manifest becomes a `plugin_load_failed` diagnostic instead of a panic.
    pub fn validate(&self) -> Result<()> {
        validate_plugin_id(&self.id)?;
        for (service, req) in &self.requires {
            validate_plugin_id(service)?;
            VersionReq::parse(req)?;
        }
        for entry in &self.http.allow {
            ark_http::HostPattern::parse(entry)?;
        }
        for tool in self.mcp.expose.iter().chain(self.mcp.consume.iter()) {
            if tool.trim().is_empty() {
                return Err(ArkError::validation("empty mcp entry in manifest"));
            }
        }
        if let Some(hooks) = &self.hooks {
            if hooks.hook_abi != ark_ports::HOOK_ABI {
                return Err(ArkError::validation(format!(
                    "unsupported hook_abi {} (host speaks {})",
                    hooks.hook_abi,
                    ark_ports::HOOK_ABI
                )));
            }
            if hooks.points.is_empty() {
                return Err(ArkError::validation("hooks section without any hook point"));
            }
        }
        Ok(())
    }

    /// Hook points the manifest declares; empty when it declares none.
    pub fn hook_points(&self) -> Vec<String> {
        self.hooks
            .as_ref()
            .map(|section| section.points.clone())
            .unwrap_or_default()
    }

    /// Parsed requirements, in deterministic order.
    pub fn requirements(&self) -> Result<Vec<(String, VersionReq)>> {
        self.requires
            .iter()
            .map(|(service, req)| Ok((service.clone(), VersionReq::parse(req)?)))
            .collect()
    }

    /// Does the manifest declare the capability?
    pub fn declares(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_roundtrips_and_orders() {
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(v.to_string(), "1.2.3");
        assert!(Version::parse("1.2.3-alpha").is_ok());
        assert!(Version::new(2, 0, 0) > Version::new(1, 9, 9));
        assert!(Version::parse("1").is_err());
        assert!(Version::parse("x.y.z").is_err());
    }

    #[test]
    fn major_version_must_match() {
        let req = VersionReq::parse("^1").unwrap();
        assert!(req.accepts(&Version::new(1, 0, 0)));
        assert!(req.accepts(&Version::new(1, 9, 9)));
        assert!(!req.accepts(&Version::new(2, 0, 0)));
        assert!(!req.accepts(&Version::new(0, 9, 0)));
    }

    #[test]
    fn minor_requirement_is_a_floor() {
        let req = VersionReq::parse("^1.4").unwrap();
        assert!(req.accepts(&Version::new(1, 4, 0)));
        assert!(req.accepts(&Version::new(1, 5, 0)));
        assert!(!req.accepts(&Version::new(1, 3, 9)));
    }

    #[test]
    fn unsupported_requirement_syntax_is_rejected() {
        for bad in ["", "~1.2", ">=1", "1.x", "*"] {
            assert!(VersionReq::parse(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn manifest_validation_catches_bad_ids_and_allowlists() {
        assert!(PluginManifest::new("Ark.Bad", "1.0.0", PluginKind::Tool).is_err());
        let mut manifest =
            PluginManifest::new("ark.example.jira", "1.0.0", PluginKind::Tool).unwrap();
        manifest.http.allow = vec!["https://api.example.com".into()];
        assert!(manifest.validate().is_err(), "URL in allowlist must fail");
        manifest.http.allow = vec!["api.example.com".into()];
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn the_hooks_section_is_an_additive_optional_request() {
        let mut manifest =
            PluginManifest::new("ark.example.life", "1.0.0", PluginKind::Backend).unwrap();
        manifest.hooks = Some(HookSection {
            hook_abi: ark_ports::HOOK_ABI,
            points: vec!["llm.request".into(), "tool.pre_call".into()],
        });
        let json = serde_json::to_value(&manifest).unwrap();
        assert_eq!(json["hooks"]["hook_abi"], ark_ports::HOOK_ABI);
        let back: PluginManifest = serde_json::from_value(json).unwrap();
        assert_eq!(back, manifest);
        assert_eq!(back.hook_points(), vec!["llm.request", "tool.pre_call"]);

        // A manifest without the section must not grow a hooks field on the
        // wire, and an old manifest must keep parsing.
        let plain = PluginManifest::new("ark.example.plain", "1.0.0", PluginKind::Tool).unwrap();
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("hooks").is_none());
        let legacy =
            r#"{"id":"ark.example.old","version":{"major":1,"minor":0,"patch":0},"kind":"tool"}"#;
        let parsed: PluginManifest = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.hooks, None);
        assert!(parsed.hook_points().is_empty());
    }

    #[test]
    fn the_hooks_section_is_validated_fail_closed() {
        let mut manifest =
            PluginManifest::new("ark.example.life", "1.0.0", PluginKind::Backend).unwrap();
        manifest.hooks = Some(HookSection {
            hook_abi: ark_ports::HOOK_ABI + 1,
            points: vec!["llm.request".into()],
        });
        assert!(
            manifest.validate().is_err(),
            "an unknown ABI must be refused"
        );
        manifest.hooks = Some(HookSection {
            hook_abi: ark_ports::HOOK_ABI,
            points: Vec::new(),
        });
        assert!(
            manifest.validate().is_err(),
            "an empty point list declares nothing"
        );
    }

    #[test]
    fn manifest_serializes_with_contract_capabilities() {
        let mut manifest =
            PluginManifest::new("ark.example.jira", "1.0.0", PluginKind::Tool).unwrap();
        manifest.capabilities = vec![Capability::Tool];
        manifest.mcp.expose = vec!["search".into()];
        let json = serde_json::to_value(&manifest).unwrap();
        assert_eq!(json["kind"], "tool");
        assert_eq!(json["capabilities"][0], "tool");
        assert_eq!(json["version"]["major"], 1);
        let back: PluginManifest = serde_json::from_value(json).unwrap();
        assert_eq!(back, manifest);
    }
}
