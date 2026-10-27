//! Incremental assembly of a stream (design §7.2 `common/stream.rs`).
//!
//! The accumulator owns the *only* mutable stream state: text so far, the
//! fragments of each in-flight tool call, and the last usage sample. Providers
//! parse bytes into [`StreamEvent`]; this type turns those into the publishable
//! [`ModelChunk`] currency of the model port.
//!
//! Nothing here buffers the whole reply: text is forwarded as it arrives, and a
//! tool call is only assembled from its own fragments.

use std::collections::BTreeMap;

use ark_common::Usage;
use ark_ports::{ModelChunk, ModelResponse};

use crate::common::repair::parse_tool_arguments;
use crate::domain::event::{StreamEvent, ToolFragment};

/// A tool call being assembled from fragments.
#[derive(Debug, Clone, Default)]
struct PendingCall {
    call_id: String,
    name: String,
    arguments: String,
    /// True once the call has been published as a chunk. The entry is kept so
    /// [`StreamAccumulator::response`] still reports it; only the *publishing*
    /// is deduplicated.
    published: bool,
}

impl PendingCall {
    fn is_complete(&self) -> bool {
        !self.call_id.is_empty() && !self.name.is_empty()
    }
}

/// Folds `StreamEvent`s into chunks and a final response.
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    text: String,
    reasoning: String,
    calls: BTreeMap<u32, PendingCall>,
    usage: Option<Usage>,
    finished: bool,
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one event, returning the chunks that may be published right now.
    pub fn push(&mut self, event: StreamEvent) -> Vec<ModelChunk> {
        match event {
            StreamEvent::Text(delta) => {
                if delta.is_empty() {
                    return Vec::new();
                }
                self.text.push_str(&delta);
                vec![ModelChunk::TextDelta { delta }]
            }
            StreamEvent::Reasoning(delta) => {
                if delta.is_empty() {
                    return Vec::new();
                }
                self.reasoning.push_str(&delta);
                vec![ModelChunk::ReasoningDelta { delta }]
            }
            StreamEvent::Tool(fragment) => self.push_fragment(fragment),
            StreamEvent::Usage(usage) => {
                self.usage = Some(usage.clone());
                vec![ModelChunk::Usage { usage }]
            }
            StreamEvent::Terminal => {
                let mut chunks = self.flush_calls();
                self.finished = true;
                chunks.shrink_to_fit();
                chunks
            }
            StreamEvent::Ignored => Vec::new(),
        }
    }

    /// Complete every tool call still being assembled.
    ///
    /// Called on a terminal event and once more when the stream ends, so a
    /// provider that closes without a sentinel cannot strand a call.
    pub fn finish(&mut self) -> Vec<ModelChunk> {
        let chunks = self.flush_calls();
        self.finished = true;
        chunks
    }

    /// The response assembled so far. `tool_calls` are `(call_id, tool, args)`.
    pub fn response(&self) -> ModelResponse {
        ModelResponse {
            text: self.text.clone(),
            tool_calls: self
                .calls
                .values()
                .filter(|call| call.is_complete())
                .map(|call| {
                    (
                        call.call_id.clone(),
                        call.name.clone(),
                        parse_tool_arguments(&call.arguments),
                    )
                })
                .collect(),
            usage: self.usage.clone().unwrap_or_default(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn reasoning(&self) -> &str {
        &self.reasoning
    }

    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    fn push_fragment(&mut self, fragment: ToolFragment) -> Vec<ModelChunk> {
        // A fragment that opens a *new* call (fresh id) means the previous calls
        // at higher indexes are done; flush them first so chunks stay ordered.
        let mut chunks = Vec::new();
        if fragment.call_id.is_some() {
            let started = self
                .calls
                .iter()
                .filter(|(index, call)| **index > fragment.index && call.is_complete())
                .map(|(index, _)| *index)
                .collect::<Vec<_>>();
            for index in started {
                if let Some(chunk) = self.flush_one(index) {
                    chunks.push(chunk);
                }
            }
        }
        let entry = self.calls.entry(fragment.index).or_default();
        if let Some(call_id) = fragment.call_id {
            entry.call_id = call_id;
        }
        if let Some(name) = fragment.name {
            entry.name = name;
        }
        entry.arguments.push_str(&fragment.arguments_delta);
        chunks
    }

    fn flush_one(&mut self, index: u32) -> Option<ModelChunk> {
        let call = self.calls.get_mut(&index)?;
        if !call.is_complete() || call.published {
            return None;
        }
        call.published = true;
        Some(ModelChunk::ToolCall {
            call_id: call.call_id.clone(),
            tool: call.name.clone(),
            arguments: parse_tool_arguments(&call.arguments),
        })
    }

    fn flush_calls(&mut self) -> Vec<ModelChunk> {
        let indexes: Vec<u32> = self.calls.keys().copied().collect();
        indexes
            .into_iter()
            .filter_map(|index| self.flush_one(index))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragment(index: u32) -> ToolFragment {
        ToolFragment::new(index)
    }

    #[test]
    fn text_deltas_are_forwarded_incrementally() {
        let mut acc = StreamAccumulator::new();
        let first = acc.push(StreamEvent::Text("he".into()));
        let second = acc.push(StreamEvent::Text("llo".into()));
        assert_eq!(first, vec![ModelChunk::TextDelta { delta: "he".into() }]);
        assert_eq!(
            second,
            vec![ModelChunk::TextDelta {
                delta: "llo".into()
            }]
        );
        assert_eq!(acc.text(), "hello");
        assert_eq!(acc.response().text, "hello");
    }

    #[test]
    fn empty_deltas_are_dropped() {
        let mut acc = StreamAccumulator::new();
        assert!(acc.push(StreamEvent::Text(String::new())).is_empty());
        assert!(acc.push(StreamEvent::Reasoning(String::new())).is_empty());
    }

    #[test]
    fn reasoning_is_kept_separate_from_text() {
        let mut acc = StreamAccumulator::new();
        let chunks = acc.push(StreamEvent::Reasoning("thinking".into()));
        assert_eq!(
            chunks,
            vec![ModelChunk::ReasoningDelta {
                delta: "thinking".into()
            }]
        );
        assert_eq!(acc.reasoning(), "thinking");
        assert_eq!(acc.text(), "");
    }

    #[test]
    fn a_tool_call_is_assembled_from_fragments() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamEvent::Tool(
            fragment(0).with_call_id("c1").with_name("read"),
        ));
        assert!(acc
            .push(StreamEvent::Tool(fragment(0).with_arguments("{\"pa")))
            .is_empty());
        let chunks = acc.push(StreamEvent::Tool(fragment(0).with_arguments("th\":\"a\"}")));
        // Still incomplete until a flush point.
        assert!(chunks.is_empty());
        let flushed = acc.finish();
        assert_eq!(flushed.len(), 1);
        match &flushed[0] {
            ModelChunk::ToolCall {
                call_id,
                tool,
                arguments,
            } => {
                assert_eq!(call_id, "c1");
                assert_eq!(tool, "read");
                assert_eq!(arguments["path"], "a");
            }
            other => panic!("expected tool call, got {other:?}"),
        }
        assert_eq!(acc.response().tool_calls.len(), 1);
    }

    #[test]
    fn terminal_event_flushes_calls() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamEvent::Tool(
            fragment(0).with_call_id("c1").with_name("read"),
        ));
        acc.push(StreamEvent::Tool(fragment(0).with_arguments("{}")));
        let chunks = acc.push(StreamEvent::Terminal);
        assert_eq!(chunks.len(), 1);
        assert!(acc.is_finished());
    }

    #[test]
    fn finishing_twice_does_not_duplicate_calls() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamEvent::Tool(
            fragment(0).with_call_id("c1").with_name("read"),
        ));
        assert_eq!(acc.push(StreamEvent::Terminal).len(), 1);
        assert!(acc.finish().is_empty());
    }

    #[test]
    fn two_parallel_calls_are_both_flushed() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamEvent::Tool(
            fragment(0).with_call_id("c1").with_name("read"),
        ));
        acc.push(StreamEvent::Tool(
            fragment(1).with_call_id("c2").with_name("grep"),
        ));
        acc.push(StreamEvent::Tool(fragment(0).with_arguments("{}")));
        acc.push(StreamEvent::Tool(fragment(1).with_arguments("{}")));
        let chunks = acc.finish();
        assert_eq!(chunks.len(), 2);
        let calls = acc.response().tool_calls;
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "c1");
        assert_eq!(calls[1].0, "c2");
    }

    #[test]
    fn usage_is_folded_and_forwarded() {
        let mut acc = StreamAccumulator::new();
        let usage = Usage::new(10, 5);
        let chunks = acc.push(StreamEvent::Usage(usage.clone()));
        assert_eq!(
            chunks,
            vec![ModelChunk::Usage {
                usage: usage.clone()
            }]
        );
        assert_eq!(acc.response().usage, usage);
    }

    #[test]
    fn ignored_events_produce_nothing() {
        let mut acc = StreamAccumulator::new();
        assert!(acc.push(StreamEvent::Ignored).is_empty());
    }

    #[test]
    fn an_incomplete_call_is_not_published() {
        let mut acc = StreamAccumulator::new();
        // No call id => cannot be dispatched.
        acc.push(StreamEvent::Tool(
            fragment(0).with_name("read").with_arguments("{}"),
        ));
        assert!(acc.finish().is_empty());
        assert!(acc.response().tool_calls.is_empty());
    }

    #[test]
    fn malformed_arguments_never_panic() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamEvent::Tool(
            fragment(0).with_call_id("c1").with_name("read"),
        ));
        acc.push(StreamEvent::Tool(fragment(0).with_arguments("{not json")));
        let chunks = acc.finish();
        assert_eq!(chunks.len(), 1);
        assert!(acc.response().tool_calls[0].2.is_object());
    }
}
