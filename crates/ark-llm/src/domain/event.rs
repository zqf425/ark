//! Provider-neutral stream events and request/response domain types.
//!
//! A provider parser does not emit `ark_ports::ModelChunk` directly: tool
//! arguments arrive as fragments, and a `ToolCall` chunk must be complete. The
//! provider layer therefore emits [`StreamEvent`] and the accumulator decides
//! when a value is publishable.

use ark_common::Usage;

/// One parsed fragment of a provider stream.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Text(String),
    Reasoning(String),
    /// Tool-call fragment. `index` identifies the call inside this stream.
    Tool(ToolFragment),
    Usage(Usage),
    /// The provider said the stream is over (`[DONE]`, `message_stop`, …).
    Terminal,
    /// A frame that carries no content we care about (role markers, pings).
    Ignored,
}

/// Partial tool call as it streams in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolFragment {
    pub index: u32,
    /// Present on the first fragment of a call.
    pub call_id: Option<String>,
    /// Present on the first fragment of a call.
    pub name: Option<String>,
    /// JSON text fragment; the full text is assembled by the accumulator.
    pub arguments_delta: String,
}

impl ToolFragment {
    pub fn new(index: u32) -> Self {
        Self {
            index,
            ..Self::default()
        }
    }

    pub fn with_call_id(mut self, call_id: impl Into<String>) -> Self {
        self.call_id = Some(call_id.into());
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_arguments(mut self, delta: impl Into<String>) -> Self {
        self.arguments_delta = delta.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_builder_sets_only_what_is_given() {
        let fragment = ToolFragment::new(1).with_call_id("c1");
        assert_eq!(fragment.index, 1);
        assert_eq!(fragment.call_id.as_deref(), Some("c1"));
        assert!(fragment.name.is_none());
        assert!(fragment.arguments_delta.is_empty());
    }

    #[test]
    fn stream_events_compare_structurally() {
        assert_eq!(StreamEvent::Text("a".into()), StreamEvent::Text("a".into()));
        assert_ne!(
            StreamEvent::Text("a".into()),
            StreamEvent::Reasoning("a".into())
        );
    }
}
