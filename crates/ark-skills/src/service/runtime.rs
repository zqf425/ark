//! Skill runtime: the scan snapshot plus the four read views the HTTP layer
//! needs (diagnostics, server list, thread view, per-turn resolution).
//!
//! The runtime is `Send + Sync` and is meant to be held in an `Arc` by the
//! server: [`SkillRuntime::new`] scans once, [`SkillRuntime::reload`] replaces
//! the snapshot, and every other method is a pure read of the current snapshot.
//! `resolve_for_turn` therefore never takes a write lock and cannot block a turn
//! behind a rescan.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use ark_contract::{
    SkillDiagnostics, SkillLoadWarning, SkillSource, SkillSummary, SkillTurnResolution,
    ThreadSkillsResponse,
};

use crate::config::{expand_home, SkillsConfig};
use crate::domain::skill::{path_display, LoadedSkill};
use crate::service::activation::{self, Match};
use crate::service::catalog::{fit_to_budget, render_catalog};
use crate::service::discovery::{self, ScanOutcome};

/// One scan result. Replaced wholesale by [`SkillRuntime::reload`].
///
/// A base scan has no separate `warnings` channel on the wire
/// (`SkillDiagnostics` only carries `validation_errors`), so every recoverable
/// problem found while discovering base skills is folded into
/// `validation_errors`, exactly like the TypeScript runtime does.
#[derive(Debug, Default, Clone)]
struct Snapshot {
    skills: Vec<LoadedSkill>,
    validation_errors: Vec<SkillLoadWarning>,
}

/// Skill discovery, validation, activation and catalog rendering.
#[derive(Debug)]
pub struct SkillRuntime {
    config: SkillsConfig,
    workspace: PathBuf,
    snapshot: RwLock<Snapshot>,
}

impl SkillRuntime {
    /// Build a runtime for `workspace` and scan once.
    ///
    /// A workspace that does not exist is not an error: only the configured
    /// absolute roots are scanned, and the workspace-relative defaults are
    /// skipped when absent (an absent default directory is normal, not a
    /// warning).
    pub fn new(config: SkillsConfig, workspace: PathBuf) -> Self {
        let runtime = Self {
            config,
            workspace,
            snapshot: RwLock::new(Snapshot::default()),
        };
        let outcome = runtime.scan();
        *runtime.write() = outcome;
        runtime
    }

    /// Rescan every configured root and replace the snapshot.
    ///
    /// Failures never abort the scan: each one becomes a warning or a
    /// validation error in the returned diagnostics.
    pub fn reload(&self) -> SkillDiagnostics {
        let outcome = self.scan();
        *self.write() = outcome;
        self.diagnostics()
    }

    /// Current diagnostics without rescanning.
    pub fn diagnostics(&self) -> SkillDiagnostics {
        let snapshot = self.read();
        SkillDiagnostics {
            enabled: self.config.enabled,
            roots: self.display_roots(),
            skills: snapshot.skills.iter().map(LoadedSkill::summary).collect(),
            validation_errors: snapshot.validation_errors.clone(),
        }
    }

    /// Server-level view: base skills only, never a thread's extras.
    pub fn list(&self) -> SkillDiagnostics {
        self.diagnostics()
    }

    /// Per-thread view: base skills plus the thread's own extra roots.
    ///
    /// An extra root that is missing, unreadable, or yields no package becomes
    /// an entry in `warnings`; the remaining roots still load. An extra skill
    /// deliberately wins over a base skill with the same id, and the shadowed
    /// base package is reported so nothing is replaced silently.
    pub fn thread_view(&self, thread_extra_roots: &[PathBuf]) -> ThreadSkillsResponse {
        if !self.config.enabled {
            return ThreadSkillsResponse {
                enabled: false,
                ..ThreadSkillsResponse::default()
            };
        }
        let snapshot = self.read();
        let mut extra_skills: Vec<LoadedSkill> = Vec::new();
        let mut warnings: Vec<SkillLoadWarning> = Vec::new();
        let mut loaded_roots: Vec<PathBuf> = Vec::new();
        let mut configured_roots: Vec<PathBuf> = Vec::new();
        for raw in thread_extra_roots {
            let raw = raw.to_string_lossy().trim().to_string();
            if raw.is_empty() {
                continue;
            }
            let configured = self.resolve_extra_root(Path::new(&raw));
            if configured_roots.iter().any(|seen| seen == &configured) {
                continue;
            }
            configured_roots.push(configured.clone());
            if !configured.is_dir() {
                warnings.push(warning(
                    &configured,
                    format!(
                        "extra skill root is not readable: {}",
                        path_display(&configured)
                    ),
                ));
                continue;
            }
            let outcome = discovery::scan_roots(&[configured.clone()], SkillSource::ThreadExtra);
            // Validation errors and warnings both surface here: this view has a
            // `warnings` list, so nothing found while loading extras is lost.
            warnings.extend(outcome.warnings);
            warnings.extend(outcome.validation_errors);
            if outcome.skills.is_empty() {
                if !warnings
                    .iter()
                    .any(|seen| seen.root == path_display(&configured))
                {
                    warnings.push(warning(
                        &configured,
                        format!(
                            "no skill packages found in {} (expected skill.json or SKILL.md)",
                            path_display(&configured)
                        ),
                    ));
                }
                continue;
            }
            loaded_roots.push(configured.clone());
            for skill in outcome.skills {
                if let Some(base) = snapshot.skills.iter().find(|base| base.id == skill.id) {
                    warnings.push(warning(
                        &skill.root,
                        format!(
                            "extra skill `{}` overrides base package at {}",
                            skill.id,
                            path_display(&base.root)
                        ),
                    ));
                }
                extra_skills.retain(|existing| existing.id != skill.id);
                extra_skills.push(skill);
            }
        }
        extra_skills.sort_by(|a, b| a.id.cmp(&b.id));
        ThreadSkillsResponse {
            enabled: true,
            base_roots: self.display_roots(),
            extra_roots: configured_roots
                .iter()
                .map(|root| path_display(root))
                .collect(),
            loaded_extra_roots: loaded_roots.iter().map(|root| path_display(root)).collect(),
            base_skills: snapshot.skills.iter().map(LoadedSkill::summary).collect(),
            extra_skills: extra_skills.iter().map(LoadedSkill::summary).collect(),
            warnings,
            base_validation_errors: snapshot.validation_errors.clone(),
        }
    }

    /// Skills this turn activates, the instruction text to inject, the catalog
    /// of the remaining skills, and the tool allow-list.
    ///
    /// Scoring and the `promptPatterns` substring simplification are documented
    /// on [`crate::service::activation`]. A skill whose entry file could not be
    /// read is skipped (with a log line) instead of aborting the turn.
    pub fn resolve_for_turn(&self, prompt: &str) -> SkillTurnResolution {
        if !self.config.enabled {
            return SkillTurnResolution::default();
        }
        let snapshot = self.read();
        let ranked = activation::match_skills(&snapshot.skills, prompt);
        let active: Vec<Match> = ranked.into_iter().take(self.config.active_limit).collect();
        let injection = inject(&active, self.config.instruction_budget_bytes);
        let injected_matches: Vec<Match> = injection
            .injected
            .iter()
            .filter_map(|index| active.get(*index).cloned())
            .collect();
        let active_ids: Vec<String> = injected_matches
            .iter()
            .map(|matched| matched.skill.id.clone())
            .collect();
        let catalog = render_catalog(
            &inactive_skills(&snapshot.skills, &active_ids),
            self.config.catalog_budget_bytes,
        );
        SkillTurnResolution {
            active_skill_ids: active_ids,
            activations: active
                .iter()
                .map(|matched| ark_contract::SkillActivation {
                    skill_id: matched.skill.id.clone(),
                    reason: matched.reason.clone(),
                    score: matched.score,
                })
                .collect(),
            instructions: injection.instructions,
            allowed_tool_names: activation::intersect_allowed_tools(&injected_matches),
            injected_bytes: injection.bytes,
            catalog,
        }
    }

    // -- internals ---------------------------------------------------------

    fn scan(&self) -> Snapshot {
        if !self.config.enabled {
            return Snapshot::default();
        }
        let outcome: ScanOutcome = discovery::scan_roots(&self.scanned_roots(), SkillSource::Base);
        let mut validation_errors = outcome.validation_errors;
        validation_errors.extend(outcome.warnings);
        Snapshot {
            skills: outcome.skills,
            validation_errors,
        }
    }

    /// Roots that actually take part in a scan.
    ///
    /// Configured roots are always included, so a typo surfaces as a warning.
    /// Workspace-relative defaults are dropped when the directory is absent:
    /// most workspaces have no `skills/` directory at all, and reporting that on
    /// every `GET /v1/skills` would be noise rather than signal.
    fn scanned_roots(&self) -> Vec<PathBuf> {
        let all = self.config.base_roots(&self.workspace);
        let optional: Vec<PathBuf> = self
            .config
            .workspace_dirs
            .iter()
            .map(|dir| expand_home(&self.workspace.join(dir.trim())))
            .collect();
        all.into_iter()
            .filter(|root| !optional.iter().any(|seen| seen == root) || root.is_dir())
            .collect()
    }

    fn display_roots(&self) -> Vec<String> {
        self.scanned_roots()
            .iter()
            .map(|root| path_display(root))
            .collect()
    }

    /// Resolve one configured extra root against the workspace.
    fn resolve_extra_root(&self, raw: &Path) -> PathBuf {
        let expanded = expand_home(raw);
        if expanded.is_absolute() {
            expanded
        } else {
            self.workspace.join(expanded)
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Snapshot> {
        match self.snapshot.read() {
            Ok(guard) => guard,
            // A poisoned lock means a previous scan panicked while holding the
            // write guard. The snapshot is replaced wholesale and never left
            // half-updated, so reading through the poison is safe here.
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Snapshot> {
        match self.snapshot.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Instructions actually injected for the active skills.
struct Injection {
    instructions: Vec<String>,
    /// Indices into the sliced active list that made it into `instructions`.
    injected: Vec<usize>,
    bytes: usize,
}

fn inject(active: &[Match], budget_bytes: usize) -> Injection {
    let mut injection = Injection {
        instructions: Vec::new(),
        injected: Vec::new(),
        bytes: 0,
    };
    let mut remaining = budget_bytes;
    for (index, matched) in active.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        let Some(text) = instruction_text(matched) else {
            tracing::warn!(
                skill = %matched.skill.id,
                entry = %path_display(&matched.skill.entry_path),
                "skill entry text is unavailable; skipping instruction injection"
            );
            continue;
        };
        let fitted = fit_to_budget(&text, remaining);
        if fitted.is_empty() {
            continue;
        }
        remaining -= fitted.len();
        injection.bytes += fitted.len();
        injection.instructions.push(fitted.to_string());
        injection.injected.push(index);
    }
    injection
}

/// Header + entry body for one activated skill.
///
/// `None` when the package was listed but its entry file could not be read.
fn instruction_text(matched: &Match) -> Option<String> {
    let entry = matched.skill.entry_text.as_deref()?;
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!(
        "Active Skill: {} ({})",
        matched.skill.name, matched.skill.id
    ));
    parts.push(format!("Activation: {}", matched.reason));
    if let Some(description) = matched.skill.description.as_deref() {
        if !description.trim().is_empty() {
            parts.push(format!("Description: {description}"));
        }
    }
    if !matched.skill.allowed_tools.is_empty() {
        parts.push(format!(
            "Allowed tools: {}",
            matched.skill.allowed_tools.join(", ")
        ));
    }
    parts.push(entry.to_string());
    Some(parts.join("\n\n"))
}

/// Discoverable skills that this turn did not activate.
fn inactive_skills(skills: &[LoadedSkill], active_ids: &[String]) -> Vec<LoadedSkill> {
    skills
        .iter()
        .filter(|skill| !active_ids.iter().any(|id| id == &skill.id))
        .cloned()
        .collect()
}

fn warning(root: &Path, message: impl Into<String>) -> SkillLoadWarning {
    SkillLoadWarning {
        root: path_display(root),
        message: message.into(),
    }
}

/// Convenience for the server: `true` when at least one base skill loaded.
impl SkillRuntime {
    /// Number of base skills in the current snapshot.
    pub fn count(&self) -> usize {
        self.read().skills.len()
    }

    /// Summaries of the current base skills.
    pub fn summaries(&self) -> Vec<SkillSummary> {
        self.read()
            .skills
            .iter()
            .map(LoadedSkill::summary)
            .collect()
    }

    /// Return one discovered skill's full instruction body by its stable id.
    ///
    /// The text comes from the current immutable scan snapshot, so callers do
    /// not need to rediscover a path or read arbitrary files on the model's
    /// behalf.
    pub fn instruction(&self, id: &str) -> Option<String> {
        let id = id.trim();
        self.read()
            .skills
            .iter()
            .find(|skill| skill.id == id)
            .and_then(|skill| skill.entry_text.clone())
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

    fn manifest(name: &str, id: &str, priority: i32, trigger: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"{name}","priority":{priority},"allowedTools":["read","write"],
"triggers":{{"commands":["/{trigger}"],"promptPatterns":["{trigger}"],"fileTypes":[".{trigger}"]}}}}"#
        )
    }

    fn runtime_with(dir: &Path, configure: impl FnOnce(&mut SkillsConfig)) -> SkillRuntime {
        let mut config = SkillsConfig::default();
        config.roots = vec![dir.to_path_buf()];
        config.workspace_dirs = Vec::new();
        configure(&mut config);
        SkillRuntime::new(config, dir.to_path_buf())
    }

    #[test]
    fn runtime_is_send_and_sync() {
        fn assert_shareable<T: Send + Sync>() {}
        assert_shareable::<SkillRuntime>();
    }

    #[test]
    fn scans_a_valid_skill_json_package() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("pdf");
        write(
            &package.join("skill.json"),
            &manifest("PDF", "pdf", 2, "pdf"),
        );
        write(&package.join("SKILL.md"), "# PDF\n\nUse pdftotext.\n");
        let runtime = runtime_with(dir.path(), |_| {});
        let diagnostics = runtime.diagnostics();
        assert!(diagnostics.enabled);
        assert_eq!(diagnostics.skills.len(), 1);
        assert!(diagnostics.validation_errors.is_empty());
        assert_eq!(diagnostics.skills[0].id, "pdf");
        assert_eq!(diagnostics.skills[0].source, SkillSource::Base);
        assert!(diagnostics.skills[0].entry_path.ends_with("SKILL.md"));
        assert_eq!(runtime.list().skills.len(), 1);
        assert_eq!(runtime.count(), 1);
    }

    #[test]
    fn loads_a_skill_md_only_package_from_front_matter() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("markdown-only");
        write(
            &package.join("SKILL.md"),
            "---\nname: Markdown Only\ndescription: Reads markdown\nversion: 2.0.0\npriority: 3\ntriggers:\n  promptPatterns:\n    - markdown\n---\n\n# Body\n",
        );
        let runtime = runtime_with(dir.path(), |_| {});
        let skill = &runtime.diagnostics().skills[0];
        assert_eq!(skill.id, "markdown-only");
        assert_eq!(skill.name, "Markdown Only");
        assert_eq!(skill.version, "2.0.0");
        assert_eq!(skill.description.as_deref(), Some("Reads markdown"));
        assert!(skill.legacy, "manifest-less packages are legacy");
    }

    #[test]
    fn parses_a_skill_md_with_a_utf8_bom() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("bom");
        fs::create_dir_all(&package).unwrap();
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"---\nname: BOM Skill\n---\n\nbody\n");
        fs::write(package.join("SKILL.md"), bytes).unwrap();
        let runtime = runtime_with(dir.path(), |_| {});
        let skill = &runtime.diagnostics().skills[0];
        assert_eq!(skill.name, "BOM Skill");
        assert_eq!(skill.id, "bom");
        assert!(runtime.diagnostics().validation_errors.is_empty());
    }

    #[test]
    fn missing_roots_and_packages_only_warn() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = SkillsConfig::default();
        config.roots = vec![dir.path().join("nope"), dir.path().to_path_buf()];
        config.workspace_dirs = Vec::new();
        let runtime = SkillRuntime::new(config, dir.path().to_path_buf());
        assert_eq!(runtime.count(), 0);
        let diagnostics = runtime.diagnostics();
        assert!(diagnostics.enabled);
        assert!(diagnostics.skills.is_empty());
    }

    #[test]
    fn absent_workspace_default_dirs_are_not_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = SkillsConfig::default();
        config.roots = Vec::new();
        config.workspace_dirs = vec!["skills".to_string()];
        let runtime = SkillRuntime::new(config, dir.path().to_path_buf());
        let diagnostics = runtime.diagnostics();
        assert!(diagnostics.roots.is_empty(), "{:?}", diagnostics.roots);
        assert!(diagnostics.skills.is_empty());
    }

    #[test]
    fn reload_picks_up_new_packages() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime_with(dir.path(), |_| {});
        assert_eq!(runtime.count(), 0);
        let package = dir.path().join("late");
        write(
            &package.join("skill.json"),
            &manifest("Late", "late", 0, "late"),
        );
        write(&package.join("SKILL.md"), "body\n");
        let diagnostics = runtime.reload();
        assert_eq!(diagnostics.skills.len(), 1);
        assert_eq!(diagnostics.skills[0].id, "late");
    }

    #[test]
    fn disabled_runtime_returns_empty_views() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("pdf");
        write(
            &package.join("skill.json"),
            &manifest("PDF", "pdf", 0, "pdf"),
        );
        write(&package.join("SKILL.md"), "body\n");
        let runtime = runtime_with(dir.path(), |config| config.enabled = false);
        assert!(!runtime.diagnostics().enabled);
        assert!(runtime.diagnostics().skills.is_empty());
        let view = runtime.thread_view(&[dir.path().to_path_buf()]);
        assert!(!view.enabled);
        assert!(view.extra_skills.is_empty());
        let resolution = runtime.resolve_for_turn("/pdf");
        assert!(resolution.active_skill_ids.is_empty());
        assert!(resolution.catalog.is_none());
    }

    #[test]
    fn active_limit_caps_activations() {
        let dir = tempfile::tempdir().unwrap();
        for (index, priority) in [5, 4, 3, 2].iter().enumerate() {
            let package = dir.path().join(format!("skill-{index}"));
            write(
                &package.join("skill.json"),
                &manifest("Skill", &format!("skill-{index}"), *priority, "deploy"),
            );
            write(&package.join("SKILL.md"), "body\n");
        }
        let runtime = runtime_with(dir.path(), |config| config.active_limit = 2);
        let resolution = runtime.resolve_for_turn("/deploy now");
        assert_eq!(resolution.active_skill_ids.len(), 2);
        assert_eq!(resolution.activations.len(), 2);
        assert_eq!(resolution.active_skill_ids[0], "skill-0");
        assert_eq!(resolution.active_skill_ids[1], "skill-1");
        let catalog = resolution.catalog.expect("remaining skills are listed");
        assert!(catalog.contains("skill-3"));
        assert!(!catalog.contains("skill-0"));
    }

    #[test]
    fn instruction_budget_truncates_without_splitting_cjk() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("cn");
        write(&package.join("skill.json"), &manifest("CN", "cn", 0, "cn"));
        write(
            &package.join("SKILL.md"),
            "这是一段很长的中文技能说明，用于验证按字节预算截断时不会切断多字节字符。",
        );
        let runtime = runtime_with(dir.path(), |config| config.instruction_budget_bytes = 60);
        let resolution = runtime.resolve_for_turn("/cn");
        assert_eq!(resolution.instructions.len(), 1);
        let injected = &resolution.instructions[0];
        assert!(injected.len() <= 60, "budget respected: {}", injected.len());
        assert!(!injected.is_empty());
        assert!(!injected.contains('\u{FFFD}'));
        assert_eq!(resolution.injected_bytes, injected.len());
    }

    #[test]
    fn allowed_tools_are_the_intersection_of_active_skills() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        write(
            &first.join("skill.json"),
            &manifest("First", "first", 3, "go"),
        );
        write(&first.join("SKILL.md"), "body\n");
        let second = dir.path().join("second");
        write(
            &second.join("skill.json"),
            &manifest("Second", "second", 2, "go"),
        );
        write(&second.join("SKILL.md"), "body\n");
        let runtime = runtime_with(dir.path(), |_| {});
        let resolution = runtime.resolve_for_turn("/go");
        assert_eq!(
            resolution.allowed_tool_names,
            Some(vec!["read".to_string(), "write".to_string()])
        );

        let plain = dir.path().join("plain");
        write(
            &plain.join("skill.json"),
            r#"{"id":"plain","name":"Plain","triggers":{"commands":["/plain"]}}"#,
        );
        write(&plain.join("SKILL.md"), "body\n");
        runtime.reload();
        let single = runtime.resolve_for_turn("/plain");
        assert_eq!(single.allowed_tool_names, None);
    }

    #[test]
    fn unreadable_entry_skips_instructions_but_keeps_activation() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("broken");
        write(
            &package.join("skill.json"),
            r#"{"id":"broken","name":"Broken","entry":"MISSING.md","triggers":{"commands":["/broken"]}}"#,
        );
        let runtime = runtime_with(dir.path(), |_| {});
        let resolution = runtime.resolve_for_turn("/broken");
        assert!(resolution.instructions.is_empty());
        assert!(resolution.active_skill_ids.is_empty());
        assert_eq!(resolution.activations.len(), 1);
        assert_eq!(resolution.activations[0].skill_id, "broken");
        assert!(
            runtime.diagnostics().skills.is_empty()
                || runtime.diagnostics().skills[0].id == "broken"
        );
    }

    #[test]
    fn thread_view_adds_extra_roots_and_reports_failures() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base");
        write(
            &base.join("skill.json"),
            &manifest("Base", "base", 0, "base"),
        );
        write(&base.join("SKILL.md"), "body\n");
        let extra_root = dir.path().join("thread-extras");
        let extra = extra_root.join("extra");
        write(
            &extra.join("skill.json"),
            &manifest("Extra", "extra", 0, "extra"),
        );
        write(&extra.join("SKILL.md"), "body\n");
        let runtime = runtime_with(&base, |_| {});

        let view = runtime.thread_view(&[extra_root.clone(), dir.path().join("missing")]);
        assert!(view.enabled);
        assert_eq!(view.base_skills.len(), 1);
        assert_eq!(view.extra_skills.len(), 1);
        assert_eq!(view.extra_skills[0].source, SkillSource::ThreadExtra);
        assert_eq!(view.loaded_extra_roots.len(), 1);
        assert_eq!(view.extra_roots.len(), 2);
        assert_eq!(view.warnings.len(), 1);
        assert!(view.warnings[0].message.contains("not readable"));
    }

    #[test]
    fn extra_skill_shadows_base_skill_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base");
        write(
            &base.join("skill.json"),
            &manifest("Base", "shared", 0, "go"),
        );
        write(&base.join("SKILL.md"), "base body\n");
        let extra_root = dir.path().join("extras");
        let extra = extra_root.join("shared");
        write(
            &extra.join("skill.json"),
            &manifest("Extra", "shared", 0, "go"),
        );
        write(&extra.join("SKILL.md"), "extra body\n");
        let runtime = runtime_with(&base, |_| {});
        let view = runtime.thread_view(&[extra_root]);
        assert_eq!(view.base_skills.len(), 1);
        assert_eq!(view.extra_skills.len(), 1);
        assert_eq!(view.extra_skills[0].name, "Extra");
        assert!(view
            .warnings
            .iter()
            .any(|warning| warning.message.contains("overrides base package")));
    }

    #[test]
    fn resolve_without_matches_still_renders_the_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("pdf");
        write(
            &package.join("skill.json"),
            &manifest("PDF", "pdf", 0, "pdf"),
        );
        write(&package.join("SKILL.md"), "body\n");
        let runtime = runtime_with(dir.path(), |_| {});
        let resolution = runtime.resolve_for_turn("unrelated request");
        assert!(resolution.active_skill_ids.is_empty());
        assert!(resolution.instructions.is_empty());
        assert_eq!(resolution.injected_bytes, 0);
        let catalog = resolution.catalog.expect("catalog");
        assert!(catalog.contains("pdf"));
    }
}
