//! Context utilities: the single token estimator and history hygiene.

pub mod checkpoint;
pub mod completion;
pub mod hygiene;
pub mod token;

pub use checkpoint::ContextCheckpoint;
pub use completion::{
    auto_delivery_attempts, consecutive_auto_model_request_failures, CompletionInbox,
    MAX_AUTO_DELIVERY_ATTEMPTS,
};
pub use hygiene::{HistoryHygiene, HygieneReport};
pub use token::TokenEstimator;

#[cfg(test)]
mod checkpoint_tests {
    use super::ContextCheckpoint;
    use ark_ports::ModelMessage;

    #[test]
    fn checkpoint_keeps_goal_constraints_and_recent_decision() {
        let checkpoint = ContextCheckpoint::from_messages(&[
            ModelMessage::user("Goal: repair the parser"),
            ModelMessage::assistant("Decision: preserve the public API"),
            ModelMessage::user("Constraint: do not change the wire format"),
        ]);

        assert!(checkpoint.render().contains("Goal: repair the parser"));
        assert!(checkpoint
            .render()
            .contains("Constraint: do not change the wire format"));
        assert!(checkpoint
            .render()
            .contains("Decision: preserve the public API"));
    }

    #[test]
    fn checkpoint_keeps_short_identifiers_requested_by_the_user() {
        let checkpoint = ContextCheckpoint::from_messages(&[ModelMessage::user(
            "Remember these passwords: ZEBRA-77 and QUARTZ-99.",
        )]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("ZEBRA-77"));
        assert!(rendered.contains("QUARTZ-99"));
    }

    #[test]
    fn checkpoint_keeps_short_quoted_strings_without_treating_plain_words_as_identifiers() {
        let checkpoint = ContextCheckpoint::from_messages(&[ModelMessage::user(
            "Keep 'db-pass' and \"release-candidate\" for the deploy.",
        )]);

        let rendered = checkpoint.render();
        assert!(rendered.contains("db-pass"));
        assert!(rendered.contains("release-candidate"));
        assert!(!rendered.contains("- deploy"));
    }

    #[test]
    fn checkpoint_rejects_empty_or_overlong_quoted_strings() {
        let too_long = "a".repeat(81);
        let checkpoint =
            ContextCheckpoint::from_texts([format!("Keep '' and \"{too_long}\"").as_str()]);

        let rendered = checkpoint.render();
        assert!(!rendered.contains(&too_long));
    }

    #[test]
    fn checkpoint_merges_only_facts_from_a_generated_checkpoint() {
        let checkpoint = ContextCheckpoint::from_messages(&[ModelMessage::system(
            "[context checkpoint]\n[remembered identifiers]\n- ZEBRA-77\nGoal: preserve legacy fact\n[3 earlier messages compacted]",
        )]);

        assert_eq!(
            checkpoint.render(),
            "[context checkpoint]\n[remembered identifiers]\n- ZEBRA-77\nGoal: preserve legacy fact"
        );
    }

    #[test]
    fn checkpoint_budget_truncates_on_utf8_boundaries() {
        let checkpoint = ContextCheckpoint::from_texts([
            "Goal: 保留足够长的上下文事实，以验证预算裁剪不会截断 UTF-8 字符。",
            "Remember QUARTZ-99 and ZEBRA-77.",
        ]);

        let rendered = checkpoint.render_with_token_limit(12);
        assert!(std::str::from_utf8(rendered.as_bytes()).is_ok());
        assert!(rendered.contains("truncated"));
        assert!(crate::context::TokenEstimator::new().estimate_text(&rendered) <= 12);
    }
}
