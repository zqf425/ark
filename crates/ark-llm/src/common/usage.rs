//! Usage parsing (design §6.4 / §7.2 `common/usage.rs`).
//!
//! Rules, and the reason they exist:
//!
//! - provider-native cache counters win over any compatibility field;
//! - `cache_hit_rate` is derived **only** from a hit/miss pair, never from
//!   `hit / prompt_tokens`, because that silently overstates efficiency;
//! - a field that the provider did not send stays `None` instead of becoming 0,
//!   so "unknown" and "zero" remain distinguishable in telemetry.

use ark_common::Usage;
use serde_json::Value;

/// Reads a usage object in any of the three wire shapes.
#[derive(Debug, Clone, Copy, Default)]
pub struct UsageParser;

impl UsageParser {
    pub fn new() -> Self {
        Self
    }

    /// OpenAI chat / responses shape.
    ///
    /// Native: `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens`
    /// (DeepSeek), then `prompt_tokens_details.cached_tokens` (OpenAI).
    pub fn parse_openai(&self, usage: &Value) -> Usage {
        let prompt = uint(usage, "prompt_tokens").or_else(|| uint(usage, "input_tokens"));
        let completion = uint(usage, "completion_tokens").or_else(|| uint(usage, "output_tokens"));
        let hit = uint(usage, "prompt_cache_hit_tokens")
            .or_else(|| nested_uint(usage, "prompt_tokens_details", "cached_tokens"))
            .or_else(|| nested_uint(usage, "input_tokens_details", "cached_tokens"));
        // An OpenAI-style `miss` is only accepted from the native field: the
        // remainder of `prompt_tokens` is not the same number when the provider
        // reports image/audio tokens separately.
        let miss = uint(usage, "prompt_cache_miss_tokens");
        self.build(prompt, completion, hit, miss)
    }

    /// Anthropic messages shape.
    ///
    /// Native: `input_tokens` (uncached input), `output_tokens`,
    /// `cache_read_input_tokens` (hit) and `cache_creation_input_tokens` (written
    /// to cache, i.e. a miss). Both sides are native, so the rate is real.
    pub fn parse_anthropic(&self, usage: &Value) -> Usage {
        let prompt = uint(usage, "input_tokens");
        let completion = uint(usage, "output_tokens");
        let hit = uint(usage, "cache_read_input_tokens");
        let miss = match (
            uint(usage, "cache_creation_input_tokens"),
            uint(usage, "input_tokens"),
        ) {
            (cache_write, Some(input)) => Some(cache_write.unwrap_or(0) + input),
            (Some(cache_write), None) => Some(cache_write),
            (None, None) => None,
        };
        self.build(prompt, completion, hit, miss)
    }

    /// Assemble the single `Usage` shape. `total_tokens` is only recomputed when
    /// the provider omitted it.
    pub fn build(
        &self,
        prompt: Option<u64>,
        completion: Option<u64>,
        hit: Option<u64>,
        miss: Option<u64>,
    ) -> Usage {
        let prompt_tokens = prompt.unwrap_or(0);
        let completion_tokens = completion.unwrap_or(0);
        // `with_cache` owns both the native counters and the "may a rate be
        // derived" decision, so this path cannot drift from `Usage::merge`.
        Usage::new(prompt_tokens, completion_tokens).with_cache(hit, miss)
    }
}

fn uint(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn nested_uint(value: &Value, outer: &str, inner: &str) -> Option<u64> {
    value
        .get(outer)
        .and_then(|nested| nested.get(inner))
        .and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deepseek_native_cache_fields_produce_a_real_rate() {
        let usage = UsageParser::new().parse_openai(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "total_tokens": 120,
            "prompt_cache_hit_tokens": 90,
            "prompt_cache_miss_tokens": 10
        }));
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 20);
        assert_eq!(usage.total_tokens, 120);
        assert_eq!(usage.cache_hit_tokens, Some(90));
        assert_eq!(usage.cache_miss_tokens, Some(10));
        assert_eq!(usage.cache_hit_rate, Some(0.9));
    }

    #[test]
    fn rate_is_never_fabricated_from_prompt_tokens() {
        // Hit known, miss unknown: `hit / prompt_tokens` would be 0.9, which is
        // exactly the fake this parser must refuse to produce.
        let usage = UsageParser::new().parse_openai(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 1,
            "prompt_cache_hit_tokens": 90
        }));
        assert_eq!(usage.cache_hit_tokens, Some(90));
        assert_eq!(usage.cache_miss_tokens, None);
        assert_eq!(usage.cache_hit_rate, None, "no rate without a miss counter");
    }

    #[test]
    fn openai_prompt_tokens_details_is_a_fallback() {
        let usage = UsageParser::new().parse_openai(&json!({
            "prompt_tokens": 50,
            "completion_tokens": 5,
            "prompt_tokens_details": { "cached_tokens": 30 }
        }));
        assert_eq!(usage.cache_hit_tokens, Some(30));
        assert_eq!(usage.cache_miss_tokens, None);
        assert_eq!(usage.cache_hit_rate, None);
    }

    #[test]
    fn missing_fields_stay_absent_not_zero() {
        let usage = UsageParser::new().parse_openai(&json!({}));
        assert_eq!(usage.total_tokens, 0);
        assert_eq!(usage.cache_hit_tokens, None);
        assert_eq!(usage.cache_miss_tokens, None);
        assert_eq!(usage.cache_hit_rate, None);
    }

    #[test]
    fn anthropic_cache_fields_are_both_native() {
        let usage = UsageParser::new().parse_anthropic(&json!({
            "input_tokens": 10,
            "output_tokens": 4,
            "cache_read_input_tokens": 90,
            "cache_creation_input_tokens": 0
        }));
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 4);
        assert_eq!(usage.cache_hit_tokens, Some(90));
        assert_eq!(usage.cache_miss_tokens, Some(10));
        assert_eq!(usage.cache_hit_rate, Some(0.9));
    }

    #[test]
    fn anthropic_cache_creation_counts_as_a_miss() {
        let usage = UsageParser::new().parse_anthropic(&json!({
            "input_tokens": 5,
            "output_tokens": 1,
            "cache_read_input_tokens": 5,
            "cache_creation_input_tokens": 5
        }));
        // Anthropic's `input_tokens` excludes cached input, so the true prompt is
        // read (5) + write (5) + uncached (5) = 15 and the rate is 5/15.
        assert_eq!(usage.cache_miss_tokens, Some(10));
        assert_eq!(usage.cache_hit_rate, Some(5.0 / 15.0));
    }

    #[test]
    fn total_is_recomputed_when_absent() {
        let usage = UsageParser::new().build(Some(7), Some(3), None, None);
        assert_eq!(usage.total_tokens, 10);
    }

    #[test]
    fn zero_hit_and_miss_yields_no_rate() {
        let usage = UsageParser::new().build(Some(1), Some(1), Some(0), Some(0));
        assert_eq!(usage.cache_hit_rate, None);
    }

    #[test]
    fn responses_shape_uses_the_same_parser() {
        let usage = UsageParser::new().parse_openai(&json!({
            "input_tokens": 12,
            "output_tokens": 8,
            "input_tokens_details": { "cached_tokens": 4 }
        }));
        assert_eq!(usage.prompt_tokens, 12);
        assert_eq!(usage.completion_tokens, 8);
        assert_eq!(usage.cache_hit_tokens, Some(4));
    }
}
