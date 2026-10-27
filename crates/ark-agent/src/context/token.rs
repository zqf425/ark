//! The one and only token estimator (design §7.1 `context/`).
//!
//! Deliberately a heuristic: the runtime must never call a tokenizer per round.
//! The point of this module is not accuracy but *centralisation* — a second
//! estimate elsewhere would make the compaction decision disagree with the
//! budget check.

use ark_ports::{ModelMessage, ModelRequest, ModelRole};

/// Per-message framing overhead charged by the chat formats.
const MESSAGE_OVERHEAD_TOKENS: u64 = 4;

/// Fixed overhead charged once per request (assistant priming).
const REQUEST_OVERHEAD_TOKENS: u64 = 3;

/// Characters per token for ASCII-heavy text.
const ASCII_CHARS_PER_TOKEN: u64 = 4;

/// CJK codepoints are roughly one token each.
const WIDE_CHARS_PER_TOKEN: u64 = 1;

/// Stateless estimator. All call sites must come through here.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokenEstimator;

impl TokenEstimator {
    pub fn new() -> Self {
        Self
    }

    /// Estimate tokens for one string.
    pub fn estimate_text(&self, text: &str) -> u64 {
        let mut ascii = 0u64;
        let mut wide = 0u64;
        for ch in text.chars() {
            if ch.is_ascii() {
                ascii += 1;
            } else {
                wide += 1;
            }
        }
        let ascii_tokens = ascii.div_ceil(ASCII_CHARS_PER_TOKEN);
        let wide_tokens = wide.div_ceil(WIDE_CHARS_PER_TOKEN);
        ascii_tokens + wide_tokens
    }

    /// Estimate tokens for one message, including framing.
    pub fn estimate_message(&self, message: &ModelMessage) -> u64 {
        let mut total = self.estimate_text(&message.content) + MESSAGE_OVERHEAD_TOKENS;
        if let Some(tool_call_id) = &message.tool_call_id {
            total += self.estimate_text(tool_call_id);
        }
        if let Some(name) = &message.name {
            total += self.estimate_text(name);
        }
        total
    }

    /// Estimate the whole request prefix, tool schemas included. Schemas are
    /// serialised compactly because they travel with every request.
    pub fn estimate_request(&self, request: &ModelRequest) -> u64 {
        let messages: u64 = request
            .messages
            .iter()
            .map(|m| self.estimate_message(m))
            .sum();
        let tools: u64 = request
            .tools
            .iter()
            .map(|schema| self.estimate_text(&schema.to_string()))
            .sum();
        messages + tools + REQUEST_OVERHEAD_TOKENS
    }

    /// Does the request exceed `limit` tokens?
    pub fn exceeds(&self, request: &ModelRequest, limit: u64) -> bool {
        self.estimate_request(request) > limit
    }

    /// Number of trailing messages that fit inside `budget` tokens, counting
    /// from the end. Always keeps at least one message so a compacted history
    /// is never empty.
    pub fn tail_that_fits(&self, messages: &[ModelMessage], budget: u64) -> usize {
        let mut used = 0u64;
        let mut kept = 0usize;
        for message in messages.iter().rev() {
            let cost = self.estimate_message(message);
            if used + cost > budget && kept > 0 {
                break;
            }
            used += cost;
            kept += 1;
        }
        kept
    }

    /// True when the message is a system message (kept verbatim on compaction).
    pub fn is_system(message: &ModelMessage) -> bool {
        message.role == ModelRole::System
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> ModelMessage {
        ModelMessage::user(text)
    }

    #[test]
    fn short_text_never_estimates_zero() {
        assert_eq!(TokenEstimator::new().estimate_text("a"), 1);
        assert_eq!(TokenEstimator::new().estimate_text(""), 0);
    }

    #[test]
    fn ascii_is_four_chars_per_token() {
        assert_eq!(TokenEstimator::new().estimate_text("abcd"), 1);
        assert_eq!(TokenEstimator::new().estimate_text(&"a".repeat(9)), 3);
    }

    #[test]
    fn wide_chars_cost_about_one_token_each() {
        assert_eq!(TokenEstimator::new().estimate_text("你好世界"), 4);
    }

    #[test]
    fn message_estimate_includes_framing() {
        let estimator = TokenEstimator::new();
        assert_eq!(
            estimator.estimate_message(&user("")),
            MESSAGE_OVERHEAD_TOKENS
        );
        assert_eq!(
            estimator.estimate_message(&user("abcd")),
            MESSAGE_OVERHEAD_TOKENS + 1
        );
    }

    #[test]
    fn tool_call_id_and_name_are_counted() {
        let mut message = user("abcd");
        message.tool_call_id = Some("call_1234".to_string());
        message.name = Some("read".to_string());
        let with_fields = TokenEstimator::new().estimate_message(&message);
        let mut plain = user("abcd");
        plain.tool_call_id = None;
        plain.name = Some("read".to_string());
        assert!(with_fields > TokenEstimator::new().estimate_message(&plain));
    }

    #[test]
    fn request_estimate_adds_tools_and_overhead() {
        let estimator = TokenEstimator::new();
        let bare = ModelRequest {
            messages: vec![user("abcd")],
            ..Default::default()
        };
        let with_tool = ModelRequest {
            messages: vec![user("abcd")],
            tools: vec![serde_json::json!({ "name": "read" })],
            ..Default::default()
        };
        assert_eq!(
            estimator.estimate_request(&bare),
            MESSAGE_OVERHEAD_TOKENS + 1 + REQUEST_OVERHEAD_TOKENS
        );
        assert!(estimator.estimate_request(&with_tool) > estimator.estimate_request(&bare));
    }

    #[test]
    fn exceeds_compares_against_the_limit() {
        let estimator = TokenEstimator::new();
        let request = ModelRequest {
            messages: vec![user("abcd")],
            ..Default::default()
        };
        let estimate = estimator.estimate_request(&request);
        assert!(!estimator.exceeds(&request, estimate));
        assert!(estimator.exceeds(&request, estimate - 1));
    }

    #[test]
    fn tail_keeps_at_least_one_message() {
        let estimator = TokenEstimator::new();
        let messages = vec![user("aaaa"), user("bbbb"), user("cccc")];
        // Budget smaller than a single message still keeps one.
        assert_eq!(estimator.tail_that_fits(&messages, 0), 1);
    }

    #[test]
    fn tail_grows_with_budget() {
        let estimator = TokenEstimator::new();
        let messages: Vec<ModelMessage> = (0..5).map(|_| user(&"a".repeat(40))).collect();
        let one = estimator.estimate_message(&messages[0]);
        assert_eq!(estimator.tail_that_fits(&messages, one), 1);
        assert_eq!(estimator.tail_that_fits(&messages, one * 2), 2);
        assert_eq!(estimator.tail_that_fits(&messages, one * 100), 5);
    }

    #[test]
    fn system_detection_matches_the_role() {
        assert!(TokenEstimator::is_system(&ModelMessage::system("x")));
        assert!(!TokenEstimator::is_system(&user("x")));
    }
}
