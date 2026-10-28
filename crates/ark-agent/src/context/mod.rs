//! Context utilities: the single token estimator and history hygiene.

pub mod checkpoint;
pub mod completion;
pub mod hygiene;
pub mod token;

pub use checkpoint::ContextCheckpoint;

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
}

pub use completion::CompletionInbox;
pub use hygiene::{HistoryHygiene, HygieneReport};
pub use token::TokenEstimator;
