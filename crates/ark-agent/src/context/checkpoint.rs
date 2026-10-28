use ark_ports::ModelMessage;

/// Deterministic, model-readable state retained across a compaction boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextCheckpoint {
    entries: Vec<String>,
}

impl ContextCheckpoint {
    /// Preserve user goals and constraints plus assistant decisions verbatim.
    pub fn from_messages(messages: &[ModelMessage]) -> Self {
        Self::from_texts(messages.iter().map(|message| message.content.as_str()))
    }

    /// Build a deterministic checkpoint from durable text rows.
    pub fn from_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> Self {
        let entries = texts
            .into_iter()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .filter(|text| {
                text.starts_with("Goal:")
                    || text.starts_with("Constraint:")
                    || text.starts_with("Decision:")
            })
            .map(ToOwned::to_owned)
            .collect();
        Self { entries }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
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
