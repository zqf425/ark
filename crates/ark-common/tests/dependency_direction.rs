//! Dependency-direction gate (design §5 dependency table).
//!
//! The table in `docs/rust-rewrite-design.md` used to be prose with no
//! enforcement, which is why drift like `ark-server → ark-agent` could survive
//! for months. This test parses each workspace crate's `[dependencies]` section
//! (`dev-dependencies` are exempt — test-only fakes are allowed to reach real
//! implementations) and asserts every `ark-*` dependency is in the table.
//!
//! To change a dependency direction: update the design table first, then this
//! map, then the code. A red CI here means the table and the code disagree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("ark-common lives at crates/<name>")
        .to_path_buf()
}

/// The allowed `ark-*` dependencies per crate, mirroring the design table.
///
/// `arkd` is the composition root and may depend on every implementation; the
/// desktop shell may depend on nothing ark.
fn allowed_dependencies() -> BTreeMap<&'static str, &'static [&'static str]> {
    BTreeMap::from([
        ("ark-common", &[] as &[&str]),
        ("ark-contract", &["ark-common"]),
        ("ark-ports", &["ark-common", "ark-contract"]),
        ("ark-config", &["ark-common", "ark-contract"]),
        ("ark-store", &["ark-common", "ark-contract", "ark-ports"]),
        ("ark-schedule", &["ark-common", "ark-contract", "ark-ports"]),
        ("ark-im", &["ark-common", "ark-contract", "ark-ports"]),
        ("ark-agent", &["ark-common", "ark-contract", "ark-ports"]),
        ("ark-llm", &["ark-common", "ark-contract", "ark-ports"]),
        ("ark-tool", &["ark-common", "ark-contract", "ark-ports"]),
        ("ark-http", &["ark-common", "ark-contract", "ark-ports"]),
        ("ark-ui", &["ark-common", "ark-contract", "ark-ports"]),
        (
            "ark-mcp",
            &["ark-common", "ark-contract", "ark-ports", "ark-http"],
        ),
        (
            "ark-plugin",
            &[
                "ark-common",
                "ark-contract",
                "ark-ports",
                "ark-http",
                "ark-ui",
                "ark-mcp",
            ],
        ),
        ("ark-skills", &["ark-common", "ark-contract", "ark-ports"]),
        (
            "ark-provider",
            &["ark-common", "ark-contract", "ark-ports", "ark-llm"],
        ),
        ("ark-workspace", &["ark-common", "ark-contract"]),
        ("ark-obs", &["ark-common", "ark-contract"]),
        (
            "ark-delegation",
            &["ark-common", "ark-contract", "ark-ports"],
        ),
        ("ark-studio", &["ark-common", "ark-contract", "ark-ports"]),
        (
            "ark-server",
            &[
                "ark-common",
                "ark-contract",
                "ark-ports",
                "ark-agent",
                "ark-store",
                "ark-config",
                "ark-obs",
                "ark-schedule",
                "ark-im",
            ],
        ),
        ("arkd", &["*"]),
        ("ark-desktop", &[]),
    ])
}

/// Workspace member manifest paths, parsed from the root `Cargo.toml`.
fn workspace_members(root: &Path) -> Vec<PathBuf> {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("root Cargo.toml");
    let members = manifest
        .lines()
        .skip_while(|line| !line.starts_with("members = ["))
        .take_while(|line| !line.trim_start().starts_with(']'))
        .filter_map(|line| {
            let trimmed = line.trim().trim_end_matches(',');
            trimmed
                .strip_prefix('"')
                .map(|rest| rest.trim_end_matches('"'))
        })
        .filter(|member| !member.is_empty())
        .map(|member| root.join(member))
        .collect::<Vec<_>>();
    assert!(
        members.len() >= 20,
        "workspace member parsing failed; the root layout must have changed"
    );
    members
}

/// Package name and production `ark-*` dependencies of one manifest.
///
/// Only the `[dependencies]` section is read: the scan stops at the next
/// section header, so `dev-dependencies` and target-specific tables cannot
/// widen the production surface.
fn production_ark_dependencies(manifest_dir: &Path) -> (String, Vec<String>) {
    let manifest =
        std::fs::read_to_string(manifest_dir.join("Cargo.toml")).expect("crate Cargo.toml");
    let name = manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("name = \""))
        .and_then(|rest| rest.split('"').next())
        .expect("every crate declares a package name")
        .to_string();
    let mut dependencies = Vec::new();
    let mut in_dependencies = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_dependencies = trimmed == "[dependencies]";
            continue;
        }
        if in_dependencies && trimmed.starts_with("ark-") {
            let dep = trimmed
                .split('=')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            dependencies.push(dep);
        }
    }
    (name, dependencies)
}

#[test]
fn every_crate_matches_the_design_dependency_table() {
    let root = workspace_root();
    let allowed = allowed_dependencies();
    let mut violations = Vec::new();
    for member in workspace_members(&root) {
        let (name, dependencies) = production_ark_dependencies(&member);
        let Some(permitted) = allowed.get(name.as_str()) else {
            violations.push(format!("{name}: not in the design dependency table"));
            continue;
        };
        for dependency in &dependencies {
            let ok = permitted.contains(&dependency.as_str()) || permitted.contains(&"*");
            if !ok {
                violations.push(format!(
                    "{name} → {dependency} is not in the design table (permitted: {permitted:?})"
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "dependency directions drifted from the design table:\n{}",
        violations.join("\n")
    );
}
