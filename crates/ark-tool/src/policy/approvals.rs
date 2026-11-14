//! Approval rule table (requirements/approval-rules.md).
//!
//! Pure matching, no I/O: the composition root snapshots the config once per
//! turn and hands the table to the sandbox policy. A rule is only allowed to
//! **relax** a `NeedsApproval` decision or **deny** outright — it can never
//! tighten a read-only or workspace-write decision, because the rule table is
//! a convenience surface, not an additional authority over the autonomy level.

use ark_contract::{ApprovalAction, ApprovalRule, ApprovalSettings, UnattendedApproval};
use ark_ports::{SandboxDecision, ToolArgs};

/// Verdict of the rule table for one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// No rule matched: the caller keeps the baseline sandbox decision.
    NoMatch,
    /// A rule allowed the call: `NeedsApproval` becomes runnable.
    Allow,
    /// A rule denied the call: the decision becomes [`SandboxDecision::Deny`].
    Deny,
}

/// Decide by the ordered rule table. The first matching rule wins; an empty
/// table never matches.
pub fn decide(settings: &ApprovalSettings, tool: &str, args: &ToolArgs) -> Verdict {
    for rule in &settings.rules {
        if rule_matches(rule, tool, args) {
            return match rule.action {
                ApprovalAction::Allow => Verdict::Allow,
                ApprovalAction::Deny => Verdict::Deny,
            };
        }
    }
    Verdict::NoMatch
}

/// Whether an unattended run forwards a still-gated call or auto-denies it.
pub fn unattended_forwards(settings: &ApprovalSettings) -> bool {
    settings.on_unattended == UnattendedApproval::Allow
}

fn rule_matches(rule: &ApprovalRule, tool: &str, args: &ToolArgs) -> bool {
    if !tool_matches(rule, tool) {
        return false;
    }
    pattern_matches(&rule.pattern, matcher_input(tool, args))
}

fn tool_matches(rule: &ApprovalRule, tool: &str) -> bool {
    rule.tools.is_empty() || rule.tools.iter().any(|name| name == tool)
}

/// What the pattern sees for one call. `bash` matches its command string —
/// the case the rule table exists for — while every other tool matches its
/// serialized arguments with deterministic key order (serde_json maps via
/// BTreeMap), so `{"path":"a.txt","offset":1}` cannot drift by key order.
fn matcher_input(tool: &str, args: &ToolArgs) -> String {
    if tool == "bash" {
        return args
            .0
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
    }
    // Serialize through a stable shape: only object keys need ordering, and
    // serde_json's default feature set preserves insertion order for `Value`.
    // Sort manually so the matcher is order-insensitive without depending on
    // feature flags.
    let mut pairs: Vec<(String, String)> = match &args.0 {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(key, value)| (key.clone(), value.to_string()))
            .collect(),
        other => vec![("value".to_string(), other.to_string())],
    };
    pairs.sort();
    let body: Vec<String> = pairs
        .into_iter()
        .map(|(key, value)| format!("{key}:{value}"))
        .collect();
    format!("{{{}}}", body.join(","))
}

/// A trailing `*` is a prefix match; anything else is exact equality. An empty
/// pattern matches any input (the rule filters by tool alone). Deliberately not
/// glob/regex: an approval rule is a security boundary and its semantics must
/// be verifiable at a glance.
fn pattern_matches(pattern: &str, input: String) -> bool {
    if pattern.is_empty() {
        return true;
    }
    match pattern.strip_suffix('*') {
        Some(prefix) => input.starts_with(prefix),
        None => pattern == input,
    }
}

/// Apply a [`Verdict`] to a baseline decision, preserving the rule table's
/// authority limits: `allow` only relaxes `NeedsApproval`, `deny` always wins
/// over the baseline, and `NoMatch` keeps the baseline untouched.
pub fn apply(verdict: Verdict, baseline: SandboxDecision) -> SandboxDecision {
    match verdict {
        Verdict::NoMatch => baseline,
        Verdict::Allow => match baseline {
            SandboxDecision::NeedsApproval => SandboxDecision::Unrestricted,
            other => other,
        },
        Verdict::Deny => SandboxDecision::Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::ApprovalRule;

    fn args_json(value: serde_json::Value) -> ToolArgs {
        ToolArgs(value)
    }

    fn rule(tools: &[&str], pattern: &str, action: ApprovalAction) -> ApprovalRule {
        ApprovalRule {
            tools: tools.iter().map(|name| name.to_string()).collect(),
            pattern: pattern.to_string(),
            action,
        }
    }

    #[test]
    fn empty_table_never_matches() {
        let settings = ApprovalSettings::default();
        assert_eq!(
            decide(
                &settings,
                "bash",
                &args_json(serde_json::json!({"command": "ls"}))
            ),
            Verdict::NoMatch
        );
    }

    #[test]
    fn first_matching_rule_wins() {
        let settings = ApprovalSettings {
            rules: vec![
                rule(&["bash"], "cargo *", ApprovalAction::Deny),
                rule(&["bash"], "cargo build*", ApprovalAction::Allow),
            ],
            ..ApprovalSettings::default()
        };
        assert_eq!(
            decide(
                &settings,
                "bash",
                &args_json(serde_json::json!({"command": "cargo build"}))
            ),
            Verdict::Deny
        );
    }

    #[test]
    fn trailing_star_is_a_prefix_match() {
        let settings = ApprovalSettings {
            rules: vec![rule(&["bash"], "cargo *", ApprovalAction::Allow)],
            ..ApprovalSettings::default()
        };
        let command = |text: &str| args_json(serde_json::json!({ "command": text }));
        assert_eq!(
            decide(&settings, "bash", &command("cargo test --workspace")),
            Verdict::Allow
        );
        assert_eq!(
            decide(&settings, "bash", &command("rustup update")),
            Verdict::NoMatch
        );
        // An exact-match pattern must not prefix-match.
        let exact = ApprovalSettings {
            rules: vec![rule(&["bash"], "cargo test", ApprovalAction::Allow)],
            ..ApprovalSettings::default()
        };
        assert_eq!(
            decide(&exact, "bash", &command("cargo test --workspace")),
            Verdict::NoMatch
        );
    }

    #[test]
    fn tool_filter_restricts_the_rule() {
        let settings = ApprovalSettings {
            rules: vec![rule(&["write"], "", ApprovalAction::Allow)],
            ..ApprovalSettings::default()
        };
        assert_eq!(
            decide(
                &settings,
                "write",
                &args_json(serde_json::json!({"path": "a"}))
            ),
            Verdict::Allow
        );
        assert_eq!(
            decide(
                &settings,
                "bash",
                &args_json(serde_json::json!({"command": "x"}))
            ),
            Verdict::NoMatch
        );
    }

    #[test]
    fn empty_tool_filter_and_pattern_match_everything() {
        let settings = ApprovalSettings {
            rules: vec![rule(&[], "", ApprovalAction::Allow)],
            ..ApprovalSettings::default()
        };
        assert_eq!(
            decide(
                &settings,
                "anything",
                &args_json(serde_json::json!({"x": 1}))
            ),
            Verdict::Allow
        );
    }

    #[test]
    fn non_bash_tools_match_normalized_json_regardless_of_key_order() {
        let settings = ApprovalSettings {
            rules: vec![rule(
                &["edit"],
                r#"{new_string:"b",old*"#,
                ApprovalAction::Allow,
            )],
            ..ApprovalSettings::default()
        };
        let a = decide(
            &settings,
            "edit",
            &args_json(serde_json::json!({"path": "f", "new_string": "b", "old_string": "a"})),
        );
        let b = decide(
            &settings,
            "edit",
            &args_json(serde_json::json!({"old_string": "a", "new_string": "b", "path": "f"})),
        );
        assert_eq!(a, Verdict::Allow);
        assert_eq!(a, b, "key order must not change the verdict");
    }

    #[test]
    fn allow_only_relaxes_needs_approval() {
        assert_eq!(
            apply(Verdict::Allow, SandboxDecision::NeedsApproval),
            SandboxDecision::Unrestricted
        );
        // Read-only and write decisions keep their meaning: a rule must not be
        // able to widen the autonomy level itself.
        assert_eq!(
            apply(Verdict::Allow, SandboxDecision::ReadOnly),
            SandboxDecision::ReadOnly
        );
        assert_eq!(
            apply(Verdict::Allow, SandboxDecision::WorkspaceWrite),
            SandboxDecision::WorkspaceWrite
        );
    }

    #[test]
    fn deny_wins_over_every_baseline() {
        for baseline in [
            SandboxDecision::ReadOnly,
            SandboxDecision::WorkspaceWrite,
            SandboxDecision::Unrestricted,
            SandboxDecision::NeedsApproval,
        ] {
            assert_eq!(apply(Verdict::Deny, baseline), SandboxDecision::Deny);
        }
    }

    #[test]
    fn unattended_disposition_reads_the_setting() {
        assert!(!unattended_forwards(&ApprovalSettings::default()));
        assert!(unattended_forwards(&ApprovalSettings {
            rules: Vec::new(),
            on_unattended: UnattendedApproval::Allow,
        }));
    }
}
