//! Skill package discovery: walk the configured roots, load each candidate
//! manifest, and reduce everything to a de-duplicated, priority-ordered list.
//!
//! Every failure is recoverable. A root that cannot be read, a manifest that
//! fails validation, or a package whose entry file vanished all become a
//! [`SkillLoadWarning`] and the remaining packages still load. Nothing here
//! panics, and nothing here returns an error type, because a broken skill
//! directory must never take the server down.

use std::path::{Path, PathBuf};

use ark_common::read_text;
use ark_contract::{SkillLoadWarning, SkillManifest, SkillSource};

use crate::domain::frontmatter;
use crate::domain::skill::{path_display, slug, LoadedSkill};

/// Manifest file name inside a package directory.
const MANIFEST_FILE: &str = "skill.json";
/// Markdown entry name, also the discovery marker for a manifest-less package.
const MARKDOWN_ENTRY: &str = "SKILL.md";
const DEFAULT_ENTRY: &str = "SKILL.md";
const DEFAULT_VERSION: &str = "0.0.0";

/// Result of scanning a list of roots.
#[derive(Debug, Default)]
pub struct ScanOutcome {
    /// Packages that passed validation, sorted by id.
    pub skills: Vec<LoadedSkill>,
    /// Packages that failed manifest validation and were therefore not loaded.
    pub validation_errors: Vec<SkillLoadWarning>,
    /// Recoverable problems that still leave the package usable.
    pub warnings: Vec<SkillLoadWarning>,
}

/// Scan every root in `roots` and merge the result.
///
/// Roots are visited in order, so with equal `priority` the earlier root keeps
/// a duplicated id. Later roots still get a warning for the id they lost.
///
/// Failures are split by severity: a root that cannot be read and a package
/// with no manifest markers are recoverable (`warnings`), while a manifest that
/// exists but is invalid is reported as a `validation_errors` entry, matching
/// the "unknown/invalid field" rule in the design.
pub fn scan_roots(roots: &[PathBuf], source: SkillSource) -> ScanOutcome {
    let mut outcome = ScanOutcome::default();
    let mut loaded: Vec<LoadedSkill> = Vec::new();
    for root in roots {
        let candidates = match package_candidates(root) {
            Ok(candidates) => candidates,
            Err(message) => {
                outcome.warnings.push(warning(root, message));
                continue;
            }
        };
        for candidate in candidates {
            match load_candidate(&candidate, source) {
                Ok((skill, entry_note)) => {
                    if let Some(note) = entry_note {
                        outcome.warnings.push(warning(&skill.root, note));
                    }
                    loaded.push(skill);
                }
                Err(failure) => match failure {
                    CandidateError::Validation(message) => {
                        outcome.validation_errors.push(warning(&candidate, message))
                    }
                    CandidateError::Unusable(message) => {
                        outcome.warnings.push(warning(&candidate, message))
                    }
                },
            }
        }
    }
    let (skills, duplicates) = dedupe_by_priority(loaded);
    outcome.warnings.extend(duplicates);
    outcome.skills = skills;
    outcome
}

/// Why one candidate package could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateError {
    /// The manifest exists but violates the schema; the package is skipped.
    Validation(String),
    /// The package has no usable manifest at all; skipped with a warning.
    Unusable(String),
}

impl CandidateError {
    /// Readable message, without the severity marker.
    pub fn message(&self) -> &str {
        match self {
            CandidateError::Validation(message) => message,
            CandidateError::Unusable(message) => message,
        }
    }
}

/// Package directories under one root.
///
/// A root is accepted in both shapes the convention allows: the root itself is
/// a single package (it contains `skill.json` or `SKILL.md`), and/or its
/// immediate subdirectories are packages. Deeper nesting is not scanned, which
/// keeps a stray `node_modules` from being crawled.
pub fn package_candidates(root: &Path) -> std::result::Result<Vec<PathBuf>, String> {
    if !root.exists() {
        return Err(format!("skill root does not exist: {}", path_display(root)));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if has_marker(root) {
        candidates.push(root.to_path_buf());
    }
    let entries = std::fs::read_dir(root)
        .map_err(|err| format!("skill root is not readable: {} ({err})", path_display(root)))?;
    let mut children: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                return Err(format!(
                    "skill root is not readable: {} ({err})",
                    path_display(root)
                ))
            }
        };
        let path = entry.path();
        if is_directory(&path) && has_marker(&path) {
            children.push(path);
        }
    }
    children.sort();
    candidates.extend(children);
    Ok(candidates)
}

/// Load one package directory.
///
/// Returns the skill plus an optional non-fatal note (the entry file could not
/// be read). A package with neither `skill.json` nor `SKILL.md` is an error so
/// the caller can warn instead of silently dropping the directory.
pub fn load_candidate(
    root: &Path,
    source: SkillSource,
) -> std::result::Result<(LoadedSkill, Option<String>), CandidateError> {
    if root.join(MANIFEST_FILE).is_file() {
        load_manifest_package(root, source)
    } else if root.join(MARKDOWN_ENTRY).is_file() {
        load_markdown_package(root, source)
    } else {
        Err(CandidateError::Unusable(format!(
            "skill package has neither {MANIFEST_FILE} nor {MARKDOWN_ENTRY}: {}",
            path_display(root)
        )))
    }
}

fn load_manifest_package(
    root: &Path,
    source: SkillSource,
) -> std::result::Result<(LoadedSkill, Option<String>), CandidateError> {
    let manifest_path = root.join(MANIFEST_FILE);
    let raw = read_text(&manifest_path).map_err(|err| {
        CandidateError::Validation(format!(
            "cannot read {}: {}",
            path_display(&manifest_path),
            err.message
        ))
    })?;
    let manifest = parse_manifest(&raw).map_err(CandidateError::Validation)?;
    let entry_name = manifest.entry.trim();
    let entry_name = if entry_name.is_empty() {
        DEFAULT_ENTRY
    } else {
        entry_name
    };
    let entry_path = join_entry(root, entry_name).map_err(CandidateError::Validation)?;
    let (entry_text, note) = read_entry(&entry_path);
    let skill = LoadedSkill {
        id: slug(manifest.id.as_deref().unwrap_or(&manifest.name)),
        name: manifest.name.trim().to_string(),
        description: manifest
            .description
            .map(|value| value.trim().to_string())
            .filter(|v| !v.is_empty()),
        version: normalize_version(&manifest.version),
        root: root.to_path_buf(),
        entry_path,
        entry_text,
        triggers: manifest.triggers,
        allowed_tools: clean_list(manifest.allowed_tools),
        priority: manifest.priority,
        source,
        legacy: false,
    };
    Ok((skill, note))
}

fn load_markdown_package(
    root: &Path,
    source: SkillSource,
) -> std::result::Result<(LoadedSkill, Option<String>), CandidateError> {
    let entry_path = root.join(MARKDOWN_ENTRY);
    let (entry_text, note) = read_entry(&entry_path);
    let text = entry_text.clone().unwrap_or_default();
    let front = frontmatter::parse(&text).unwrap_or_default();
    let folder = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "skill".to_string());
    let description = front
        .description
        .clone()
        .or_else(|| frontmatter::first_paragraph(frontmatter::body(&text)));
    let skill = LoadedSkill {
        id: slug(front.id.as_deref().unwrap_or(&folder)),
        name: front.name.clone().unwrap_or_else(|| folder.clone()),
        description,
        version: normalize_version(front.version.as_deref().unwrap_or(DEFAULT_VERSION)),
        root: root.to_path_buf(),
        entry_path,
        entry_text,
        triggers: front.triggers,
        allowed_tools: clean_list(front.allowed_tools),
        priority: front.priority.unwrap_or(0),
        source,
        legacy: true,
    };
    Ok((skill, note))
}

/// Parse and validate a `skill.json` body.
///
/// The frozen contract type uses snake_case field names while the on-disk
/// convention (and every bundled package) uses camelCase, so keys are
/// normalized before deserialization. Unknown keys are rejected, mirroring the
/// strict schema in the TypeScript runtime.
fn parse_manifest(raw: &str) -> std::result::Result<SkillManifest, String> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| format!("{MANIFEST_FILE} is not valid JSON: {err}"))?;
    let serde_json::Value::Object(object) = value else {
        return Err(format!("{MANIFEST_FILE} must be a JSON object"));
    };
    let mut normalized = serde_json::Map::new();
    let mut unknown: Vec<String> = Vec::new();
    for (key, value) in object {
        let canonical = canonical_key(&key);
        match canonical {
            Some(name) => {
                let value = if name == "triggers" {
                    normalize_triggers(value)?
                } else {
                    value
                };
                normalized.insert(name.to_string(), value);
            }
            None => unknown.push(key),
        }
    }
    if !unknown.is_empty() {
        unknown.sort();
        return Err(format!(
            "unknown {MANIFEST_FILE} field(s): {}",
            unknown.join(", ")
        ));
    }
    let manifest: SkillManifest = serde_json::from_value(serde_json::Value::Object(normalized))
        .map_err(|err| format!("{MANIFEST_FILE} does not match the skill schema: {err}"))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_manifest(manifest: &SkillManifest) -> std::result::Result<(), String> {
    if manifest.name.trim().is_empty() {
        return Err(format!("{MANIFEST_FILE} is missing a non-empty `name`"));
    }
    if let Some(id) = &manifest.id {
        if id.trim().is_empty() {
            return Err(format!("{MANIFEST_FILE} has an empty `id`"));
        }
    }
    Ok(())
}

fn normalize_triggers(value: serde_json::Value) -> std::result::Result<serde_json::Value, String> {
    if value.is_null() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    let serde_json::Value::Object(object) = value else {
        return Err(format!("{MANIFEST_FILE} `triggers` must be an object"));
    };
    let mut normalized = serde_json::Map::new();
    let mut unknown: Vec<String> = Vec::new();
    for (key, value) in object {
        match key.as_str() {
            "commands" => normalized.insert("commands".to_string(), value),
            "promptPatterns" | "prompt_patterns" => {
                normalized.insert("prompt_patterns".to_string(), value)
            }
            "fileTypes" | "file_types" => normalized.insert("file_types".to_string(), value),
            _ => {
                unknown.push(key);
                None
            }
        };
    }
    if !unknown.is_empty() {
        unknown.sort();
        return Err(format!(
            "unknown {MANIFEST_FILE} `triggers` field(s): {}",
            unknown.join(", ")
        ));
    }
    Ok(serde_json::Value::Object(normalized))
}

fn canonical_key(key: &str) -> Option<&'static str> {
    match key {
        "id" => Some("id"),
        "name" => Some("name"),
        "description" => Some("description"),
        "version" => Some("version"),
        "entry" => Some("entry"),
        "triggers" => Some("triggers"),
        "allowedTools" | "allowed_tools" => Some("allowed_tools"),
        "assets" => Some("assets"),
        "priority" => Some("priority"),
        _ => None,
    }
}

/// Resolve `entry` inside the package, refusing paths that escape it.
fn join_entry(root: &Path, entry: &str) -> std::result::Result<PathBuf, String> {
    let candidate = Path::new(entry);
    let escapes = candidate.is_absolute()
        || candidate
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir));
    if escapes {
        return Err(format!(
            "skill `entry` must stay inside the package: {entry}"
        ));
    }
    Ok(root.join(candidate))
}

/// Read the entry text. A missing/unreadable entry is a warning, not a
/// rejection: the package stays listed so the user can see and repair it.
fn read_entry(path: &Path) -> (Option<String>, Option<String>) {
    match read_text(path) {
        Ok(text) => (Some(text), None),
        Err(err) => (
            None,
            Some(format!(
                "skill entry is not readable: {} ({})",
                path_display(path),
                err.message
            )),
        ),
    }
}

fn normalize_version(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        DEFAULT_VERSION.to_string()
    } else {
        trimmed.to_string()
    }
}

fn clean_list(values: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !out.iter().any(|existing| existing == trimmed) {
            out.push(trimmed.to_string());
        }
    }
    out
}

fn has_marker(dir: &Path) -> bool {
    dir.join(MANIFEST_FILE).is_file() || dir.join(MARKDOWN_ENTRY).is_file()
}

/// Directory check that follows symlinks, so a linked skill package is still
/// discovered (the shared `.agents/skills` convention links packages in).
fn is_directory(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_dir(),
        Err(_) => false,
    }
}

/// Keep the highest-`priority` package per id and warn about the losers.
fn dedupe_by_priority(skills: Vec<LoadedSkill>) -> (Vec<LoadedSkill>, Vec<SkillLoadWarning>) {
    let mut warnings: Vec<SkillLoadWarning> = Vec::new();
    let mut kept: Vec<LoadedSkill> = Vec::new();
    for skill in skills {
        match kept.iter().position(|existing| existing.id == skill.id) {
            None => kept.push(skill),
            Some(index) => {
                let existing = &kept[index];
                if skill.priority > existing.priority {
                    warnings.push(warning(
                        &existing.root,
                        format!(
                            "duplicate skill id `{}` replaced by higher priority package at {}",
                            skill.id,
                            path_display(&skill.root)
                        ),
                    ));
                    kept[index] = skill;
                } else {
                    warnings.push(warning(
                        &skill.root,
                        format!(
                            "duplicate skill id `{}` ignored in favour of {}",
                            skill.id,
                            path_display(&existing.root)
                        ),
                    ));
                }
            }
        }
    }
    kept.sort_by(|a, b| a.id.cmp(&b.id));
    (kept, warnings)
}

fn warning(root: &Path, message: impl Into<String>) -> SkillLoadWarning {
    SkillLoadWarning {
        root: path_display(root),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn loads_skill_json_packages() {
        let dir = temp_root();
        let package = dir.path().join("pdf");
        write(
            &package.join("skill.json"),
            r#"{"id":"pdf-tools","name":"PDF Tools","version":"1.2.0","allowedTools":["read"],"priority":2}"#,
        );
        write(&package.join("SKILL.md"), "# PDF\n\nUse pdftotext.\n");
        let outcome = scan_roots(&[dir.path().to_path_buf()], SkillSource::Base);
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
        assert_eq!(outcome.skills.len(), 1);
        let skill = &outcome.skills[0];
        assert_eq!(skill.id, "pdf-tools");
        assert_eq!(skill.name, "PDF Tools");
        assert_eq!(skill.version, "1.2.0");
        assert_eq!(skill.allowed_tools, vec!["read"]);
        assert_eq!(skill.priority, 2);
        assert!(!skill.legacy);
        assert!(skill.entry_text.as_deref().unwrap().contains("pdftotext"));
    }

    #[test]
    fn accepts_a_root_that_is_itself_a_package() {
        let dir = temp_root();
        write(
            &dir.path().join("skill.json"),
            r#"{"name":"Solo","version":"","triggers":{}}"#,
        );
        write(&dir.path().join("SKILL.md"), "body\n");
        let outcome = scan_roots(&[dir.path().to_path_buf()], SkillSource::Base);
        assert_eq!(outcome.skills.len(), 1);
        // Empty version falls back to the documented default.
        assert_eq!(outcome.skills[0].version, "0.0.0");
        assert_eq!(outcome.skills[0].id, "solo");
    }

    #[test]
    fn unknown_manifest_fields_are_validation_errors() {
        let dir = temp_root();
        let package = dir.path().join("bad");
        write(
            &package.join("skill.json"),
            r#"{"name":"Bad","surprise":true}"#,
        );
        write(&package.join("SKILL.md"), "body\n");
        let outcome = scan_roots(&[dir.path().to_path_buf()], SkillSource::Base);
        assert!(outcome.skills.is_empty());
        assert_eq!(outcome.validation_errors.len(), 1);
        assert!(outcome.validation_errors[0].message.contains("surprise"));
    }

    #[test]
    fn empty_name_is_rejected() {
        let dir = temp_root();
        let package = dir.path().join("nameless");
        write(&package.join("skill.json"), r#"{"name":"   "}"#);
        let outcome = scan_roots(&[dir.path().to_path_buf()], SkillSource::Base);
        assert!(outcome.skills.is_empty());
        assert!(outcome.validation_errors[0].message.contains("name"));
    }

    #[test]
    fn entry_escaping_the_package_is_rejected() {
        let dir = temp_root();
        let package = dir.path().join("escape");
        write(
            &package.join("skill.json"),
            r#"{"name":"Escape","entry":"../x.md"}"#,
        );
        let outcome = scan_roots(&[dir.path().to_path_buf()], SkillSource::Base);
        assert!(outcome.skills.is_empty());
        assert!(outcome.validation_errors[0].message.contains("entry"));
    }

    #[test]
    fn package_without_markers_reports_a_warning() {
        let dir = temp_root();
        let package = dir.path().join("empty-package");
        fs::create_dir_all(&package).unwrap();
        let result = load_candidate(&package, SkillSource::Base);
        let message = result.unwrap_err();
        assert!(message
            .message()
            .contains("neither skill.json nor SKILL.md"));
    }

    #[test]
    fn duplicate_ids_keep_the_higher_priority_package() {
        let dir = temp_root();
        let low = dir.path().join("a-low");
        write(
            &low.join("skill.json"),
            r#"{"id":"dup","name":"Low","priority":1}"#,
        );
        write(&low.join("SKILL.md"), "low\n");
        let high = dir.path().join("b-high");
        write(
            &high.join("skill.json"),
            r#"{"id":"dup","name":"High","priority":9}"#,
        );
        write(&high.join("SKILL.md"), "high\n");
        let outcome = scan_roots(&[dir.path().to_path_buf()], SkillSource::Base);
        assert_eq!(outcome.skills.len(), 1);
        assert_eq!(outcome.skills[0].name, "High");
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].message.contains("duplicate skill id"));
        assert!(outcome.warnings[0].root.ends_with("a-low"));
    }

    #[test]
    fn missing_root_is_a_warning_not_a_failure() {
        let dir = temp_root();
        let missing = dir.path().join("nope");
        let outcome = scan_roots(&[missing], SkillSource::Base);
        assert!(outcome.skills.is_empty());
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].message.contains("does not exist"));
    }

    #[test]
    fn missing_entry_file_only_warns() {
        let dir = temp_root();
        let package = dir.path().join("no-entry");
        write(
            &package.join("skill.json"),
            r#"{"name":"NoEntry","entry":"MISSING.md"}"#,
        );
        let outcome = scan_roots(&[dir.path().to_path_buf()], SkillSource::Base);
        assert_eq!(outcome.skills.len(), 1, "package stays listed");
        assert!(outcome.skills[0].entry_text.is_none());
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0]
            .message
            .contains("entry is not readable"));
    }

    #[test]
    fn skips_unrelated_files_in_a_root() {
        let dir = temp_root();
        write(&dir.path().join("README.md"), "not a package\n");
        fs::create_dir_all(dir.path().join("plain-dir")).unwrap();
        let outcome = scan_roots(&[dir.path().to_path_buf()], SkillSource::Base);
        assert!(outcome.skills.is_empty());
        assert!(outcome.warnings.is_empty());
    }
}
