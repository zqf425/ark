use ark_ports::ModelMessage;

/// Deterministic, model-readable state retained across a compaction boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextCheckpoint {
    entries: Vec<String>,
}

impl ContextCheckpoint {
    /// Preserve user goals and constraints plus assistant decisions verbatim.
    pub fn from_messages(messages: &[ModelMessage]) -> Self {
        let entries = messages
            .iter()
            .filter_map(|message| {
                let text = message.content.trim();
                (text.starts_with("Goal:")
                    || text.starts_with("Constraint:")
                    || text.starts_with("Decision:"))
                .then(|| text.to_string())
            })
            .collect();
        Self { entries }
    }

    /// Render the durable state for the next model context window.
    pub fn render(&self) -> String {
        if self.entries.is_empty() {
            "[context checkpoint: no structured facts extracted]".to_string()
        } else {
            format!("[context checkpoint]\n{}", self.entries.join("\n"))
        }
    }
}
