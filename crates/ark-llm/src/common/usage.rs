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
        // A native miss field always wins. Without one, the hit/miss pair can
        // still be completed algebraically — but only when the uncached
        // remainder is exactly `prompt - hit`. If the same usage object splits
        // image/audio tokens out of the prompt, that remainder also contains
        // billable non-cacheable modalities, so the rate stays unknown instead
        // of being fabricated (measured 2026-11-18: brgroup_bl, Nvidia and
        // openrouter all report `cached_tokens` with no miss field and no
        // split modalities; this is what turns their cache chips visible).
        let miss = uint(usage, "prompt_cache_miss_tokens").or_else(|| {
            if splits_modalities(usage) {
                return None;
            }
            match (prompt, hit) {
                (Some(prompt), Some(hit)) => Some(prompt.saturating_sub(hit)),
                _ => None,
            }
        });
        self.build(prompt, completion, hit, miss)
    }

    /// Anthropic messages shape.
    ///
    /// Native: `input_tokens` (uncached input), `output_tokens`,
    /// `cache_read_input_tokens` (hit) and `cache_creation_input_tokens` (written
    /// to cache, i.e. a miss). Both sides are native, so the rate is real.
    pub fn parse_anthropic(&self, usage: &Value) -> Usage {
        let input = uint(usage, "input_tokens");
        let completion = uint(usage, "output_tokens");
        let hit = uint(usage, "cache_read_input_tokens");
        let cache_write = uint(usage, "cache_creation_input_tokens");
        // Anthropic excludes both cache counters from `input_tokens`; all three
        // are billable prompt tokens and must be present in the aggregate total.
        let prompt = match (input, hit, cache_write) {
            (None, None, None) => None,
            _ => Some(
                input
                    .unwrap_or(0)
                    .saturating_add(hit.unwrap_or(0))
                    .saturating_add(cache_write.unwrap_or(0)),
            ),
        };
        let miss = match (cache_write, input) {
            (cache_write, Some(input)) => Some(cache_write.unwrap_or(0).saturating_add(input)),
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

/// True when the provider reports a NONZERO image/audio token count inside
/// the prompt details. In the OpenAI family `prompt_tokens` includes the
/// cached portion, so `prompt - cached` is the complete uncached remainder;
/// an explicitly zero modality (brgroup_bl ships `image_tokens: 0`) adds no
/// unaccounted tokens either. Only a nonzero split makes the remainder's
/// composition unverifiable for gateways that exclude images from `prompt`:
/// then the rate stays unknown instead of being fabricated.
fn splits_modalities(usage: &Value) -> bool {
    const DETAIL_KEYS: [&str; 2] = ["prompt_tokens_details", "input_tokens_details"];
    const MODALITY_KEYS: [&str; 2] = ["image_tokens", "audio_tokens"];
    DETAIL_KEYS.iter().any(|outer| {
        usage.get(outer).is_some_and(|details| {
            MODALITY_KEYS.iter().any(|key| {
                details
                    .get(key)
                    .and_then(Value::as_u64)
                    .is_some_and(|n| n > 0)
            })
        })
    })
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
    fn hit_without_native_miss_is_completed_by_the_known_remainder() {
        // Hit known, no native miss field, no split modalities: the uncached
        // remainder is exactly `prompt - hit` (100 - 90 = 10), so the pair is
        // complete and the rate is real. What stays forbidden is fabricating a
        // rate while the remainder is unknowable — split modalities (tested in
        // `openai_cached_tokens_does_not_guess_with_split_modalities`) or a
        // hit from `prompt_tokens_details` without a prompt total.
        let usage = UsageParser::new().parse_openai(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 1,
            "prompt_cache_hit_tokens": 90
        }));
        assert_eq!(usage.cache_hit_tokens, Some(90));
        assert_eq!(usage.cache_miss_tokens, Some(10));
        assert_eq!(usage.cache_hit_rate, Some(0.9));
    }

    #[test]
    fn openai_prompt_tokens_details_is_a_fallback() {
        let usage = UsageParser::new().parse_openai(&json!({
            "prompt_tokens": 50,
            "completion_tokens": 5,
            "prompt_tokens_details": { "cached_tokens": 30 }
        }));
        assert_eq!(usage.cache_hit_tokens, Some(30));
        // The uncached remainder 50 - 30 is fully accounted for, so the miss
        // is derived and the rate is real (brgroup_bl / Nvidia / openrouter
        // all ship this shape).
        assert_eq!(usage.cache_miss_tokens, Some(20));
        assert_eq!(usage.cache_hit_rate, Some(0.6));
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
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 4);
        assert_eq!(usage.total_tokens, 104);
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
        assert_eq!(usage.prompt_tokens, 15);
        assert_eq!(usage.total_tokens, 16);
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
    fn openai_cached_tokens_derives_a_miss_when_details_are_flat() {
        // brgroup_bl / Nvidia / openrouter report a hit via
        // `prompt_tokens_details.cached_tokens` (or the responses-shape
        // `input_tokens_details`) but no miss field. When the same usage
        // object carries no separate image/audio token reporting, the
        // uncached remainder is exactly `prompt - cached`, so the hit/miss
        // pair is complete and a real rate can be derived.
        let usage = UsageParser::new().parse_openai(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 5,
            "prompt_tokens_details": { "cached_tokens": 90 }
        }));
        assert_eq!(usage.cache_hit_tokens, Some(90));
        assert_eq!(usage.cache_miss_tokens, Some(10));
        assert_eq!(usage.cache_hit_rate, Some(0.9));
    }

    #[test]
    fn openai_cached_tokens_does_not_guess_with_nonzero_split_modalities() {
        // A nonzero image/audio count inside the prompt details makes the
        // remainder's composition unverifiable (gateways may exclude images
        // from `prompt_tokens`). Without a native miss field the rate stays
        // unknown instead of being fabricated.
        let usage = UsageParser::new().parse_openai(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 5,
            "prompt_tokens_details": { "cached_tokens": 90, "image_tokens": 7 }
        }));
        assert_eq!(usage.cache_hit_tokens, Some(90));
        assert_eq!(usage.cache_miss_tokens, None);
        assert_eq!(usage.cache_hit_rate, None);
    }

    #[test]
    fn openai_explicit_zero_modalities_do_not_block_derivation() {
        // brgroup_bl reports `image_tokens: 0` next to `cached_tokens`: an
        // explicitly zero modality contributes no unaccounted tokens, so the
        // uncached remainder is still exactly `prompt - cached`.
        let usage = UsageParser::new().parse_openai(&json!({
            "prompt_tokens": 37,
            "completion_tokens": 14,
            "prompt_tokens_details": {
                "cached_tokens": 0,
                "image_tokens": 0,
                "text_tokens": 37
            }
        }));
        assert_eq!(usage.cache_hit_tokens, Some(0));
        assert_eq!(usage.cache_miss_tokens, Some(37));
        assert_eq!(usage.cache_hit_rate, Some(0.0));
    }

    #[test]
    fn native_miss_field_still_wins_over_derivation() {
        // DeepSeek sends both native counters; the derivation must not
        // overwrite or duplicate them.
        let usage = UsageParser::new().parse_openai(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 5,
            "prompt_cache_hit_tokens": 90,
            "prompt_cache_miss_tokens": 10,
            "prompt_tokens_details": { "cached_tokens": 90 }
        }));
        assert_eq!(usage.cache_hit_tokens, Some(90));
        assert_eq!(usage.cache_miss_tokens, Some(10));
        assert_eq!(usage.cache_hit_rate, Some(0.9));
    }

    #[test]
    fn zero_cached_tokens_yields_a_zero_rate_with_full_miss() {
        // First call after cache creation: everything was written, nothing
        // read — but cached_tokens = 0 with a positive prompt means the
        // miss is the full prompt. cached = 0 must still produce a rate of
        // 0, and prompt > 0 with cached = 0 yields miss = prompt (rate 0).
        let usage = UsageParser::new().parse_openai(&json!({
            "prompt_tokens": 37,
            "completion_tokens": 13,
            "prompt_tokens_details": { "cached_tokens": 0 }
        }));
        assert_eq!(usage.cache_hit_tokens, Some(0));
        assert_eq!(usage.cache_miss_tokens, Some(37));
        assert_eq!(usage.cache_hit_rate, Some(0.0));
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
