use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;

/// The one and only usage shape (design §6.4).
///
/// Provider-native fields are preserved verbatim. `cache_hit_rate` is only
/// derived when both hit and miss counters exist: never faked from
/// `hit / prompt_tokens`, because that silently overstates efficiency.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct Usage {
    #[ts(as = "f64")]
    pub prompt_tokens: u64,
    #[ts(as = "f64")]
    pub completion_tokens: u64,
    #[ts(as = "f64")]
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub cache_hit_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub cache_miss_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hit_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Wall-clock duration of the model call this sample closes, measured
    /// by the agent loop around the provider stream. Optional because only
    /// the runtime's per-round samples set it: a provider payload parsed by
    /// `ark-llm` never carries one, and aggregates must not invent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub duration_ms: Option<u64>,
    /// Set once any folded sample omitted a native cache counter.
    ///
    /// Not serialized: it is bookkeeping for `merge`, not a wire field. Without
    /// it, folding `{hit: 90, miss: unknown}` and `{hit: 10, miss: 10}` produced
    /// `100/(100+10)` — a definite rate assembled from a value the provider never
    /// sent (design §6.4, P1-8).
    #[serde(skip)]
    cache_counters_incomplete: bool,
    /// Set once duration folding has seen both a measured and unmeasured sample.
    #[serde(skip)]
    duration_incomplete: bool,
    /// True when the rate is derived from a miss the provider never sent
    /// (`prompt - hit` past a split-modality remainder). Wire-visible so the
    /// renderer can mark the rate as approximate; sticky in `merge` like the
    /// incompleteness flags.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>")]
    pub cache_hit_rate_approximate: bool,
    /// Distinguishes an empty `Usage::default()` accumulator from a real sample.
    #[serde(skip)]
    samples_merged: bool,
}

impl Usage {
    pub fn new(prompt_tokens: u64, completion_tokens: u64) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            samples_merged: true,
            ..Default::default()
        }
    }

    pub fn with_cache(mut self, hit: Option<u64>, miss: Option<u64>) -> Self {
        self.cache_hit_tokens = hit;
        self.cache_miss_tokens = miss;
        self.cache_counters_incomplete = hit.is_none() || miss.is_none();
        self.cache_hit_rate = Self::rate(hit, miss);
        self
    }

    /// Same as `with_cache`, but marks the derived rate approximate: the miss
    /// side was reconstructed from a prompt that also contained tokens the
    /// provider bills outside the cacheable text remainder (image/audio), so
    /// `hit / (hit + miss)` overstates the hit rate by an unknown margin.
    pub fn with_cache_approximate(self, hit: Option<u64>, miss: Option<u64>) -> Self {
        self.with_cache(hit, miss).mark_approximate()
    }

    /// Flags an already-built usage's derived rate as approximate without
    /// touching its counters.
    pub fn mark_approximate(mut self) -> Self {
        self.cache_hit_rate_approximate = self.cache_hit_rate.is_some();
        self
    }

    /// `hit / (hit + miss)` with no guesswork when either side is unknown.
    pub fn rate(hit: Option<u64>, miss: Option<u64>) -> Option<f64> {
        match (hit, miss) {
            (Some(hit), Some(miss)) if hit + miss > 0 => Some(hit as f64 / (hit + miss) as f64),
            _ => None,
        }
    }

    /// Fold a later usage sample into an accumulator (turn totals).
    ///
    /// A single missing counter poisons the aggregate rate, permanently: once a
    /// sample did not report `miss`, no sum of the remaining samples can restore
    /// `hit / (hit + miss)` for the whole turn. The rate therefore becomes
    /// `None` rather than a number that looks measured but is not.
    pub fn merge(&mut self, other: &Usage) {
        let self_had_sample = self.samples_merged
            || self.prompt_tokens != 0
            || self.completion_tokens != 0
            || self.total_tokens != 0
            || self.duration_ms.is_some();
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
        self.cache_hit_tokens = sum_opt(self.cache_hit_tokens, other.cache_hit_tokens);
        self.cache_miss_tokens = sum_opt(self.cache_miss_tokens, other.cache_miss_tokens);
        self.cache_counters_incomplete = self.cache_counters_incomplete
            || other.cache_counters_incomplete
            || other.cache_hit_tokens.is_none()
            || other.cache_miss_tokens.is_none();
        self.cache_hit_rate = if self.cache_counters_incomplete {
            None
        } else {
            Usage::rate(self.cache_hit_tokens, self.cache_miss_tokens)
        };
        // Approximation is sticky for the same reason a missing counter is: a
        // mixed aggregate has no single honest label, and "approximate" is the
        // safe description of any fold that contains a guessed miss.
        self.cache_hit_rate_approximate =
            self.cache_hit_rate_approximate || other.cache_hit_rate_approximate;
        self.cost_usd = sum_opt_f(self.cost_usd, other.cost_usd);
        self.duration_incomplete = self.duration_incomplete
            || other.duration_incomplete
            || (self_had_sample && self.duration_ms.is_none())
            || other.duration_ms.is_none();
        self.duration_ms = if self.duration_incomplete {
            None
        } else {
            sum_opt(self.duration_ms, other.duration_ms)
        };
        self.samples_merged = true;
    }
}

fn sum_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (Some(a), Some(b)) => Some(a + b),
    }
}

fn sum_opt_f(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (None, None) => None,
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (Some(a), Some(b)) => Some(a + b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_requires_both_sides() {
        assert_eq!(Usage::rate(Some(90), None), None);
        assert_eq!(Usage::rate(None, None), None);
        assert_eq!(Usage::rate(Some(0), Some(0)), None);
        assert_eq!(Usage::rate(Some(90), Some(10)), Some(0.9));
    }

    #[test]
    fn merge_keeps_absent_counters_absent() {
        let mut a = Usage::new(10, 5);
        let b = Usage::new(20, 5);
        a.merge(&b);
        assert_eq!(a.total_tokens, 40);
        assert_eq!(a.cache_hit_rate, None);
    }

    #[test]
    fn merge_sums_complete_cache_counters() {
        let mut total = Usage::new(10, 1).with_cache(Some(90), Some(10));
        total.merge(&Usage::new(5, 1).with_cache(Some(30), Some(20)));
        assert_eq!(total.cache_hit_tokens, Some(120));
        assert_eq!(total.cache_miss_tokens, Some(30));
        assert_eq!(total.cache_hit_rate, Some(0.8));
    }

    #[test]
    fn merge_sums_measured_durations_and_stays_unknown_without_them() {
        // Per-round wall-clock samples fold like the cache counters: the
        // aggregate is a sum only when every sample was timed.
        let mut total = Usage::new(10, 5);
        total.duration_ms = Some(1_200);
        total.merge(&{
            let mut next = Usage::new(5, 5);
            next.duration_ms = Some(800);
            next
        });
        assert_eq!(total.duration_ms, Some(2_000));

        let mut partial = Usage::new(10, 5);
        partial.duration_ms = Some(1_200);
        partial.merge(&Usage::new(5, 5));
        assert_eq!(partial.duration_ms, None);

        let mut missing_first = Usage::new(10, 5);
        let mut measured_later = Usage::new(5, 5);
        measured_later.duration_ms = Some(800);
        missing_first.merge(&measured_later);
        assert_eq!(missing_first.duration_ms, None);

        let mut unmeasured = Usage::new(10, 5);
        unmeasured.merge(&Usage::new(5, 5));
        assert_eq!(unmeasured.duration_ms, None);
    }

    #[test]
    fn merge_never_invents_a_rate_from_a_partial_sample() {
        // Provider A reported hit but no miss; provider B reported both. The
        // combined rate is unknown, not 100/(100+10) (P1-8).
        let mut total = Usage::new(10, 1).with_cache(Some(90), None);
        total.merge(&Usage::new(5, 1).with_cache(Some(10), Some(10)));
        assert_eq!(total.cache_hit_rate, None);

        // Order must not matter: the taint is sticky.
        let mut reversed = Usage::new(5, 1).with_cache(Some(10), Some(10));
        reversed.merge(&Usage::new(10, 1).with_cache(Some(90), None));
        assert_eq!(reversed.cache_hit_rate, None);
    }

    #[test]
    fn approximate_flag_is_sticky_and_not_set_by_real_pairs() {
        // A real pair never carries the flag.
        let real = Usage::new(10, 1).with_cache(Some(90), Some(10));
        assert!(!real.cache_hit_rate_approximate);

        // A guessed miss does.
        let approx = Usage::new(10, 1).with_cache_approximate(Some(90), Some(10));
        assert!(approx.cache_hit_rate_approximate);
        assert_eq!(approx.cache_hit_rate, Some(0.9));

        // Mixing one approximate sample into an aggregate marks the whole
        // total approximate: the label must describe the worst sample.
        let mut total = Usage::new(10, 1).with_cache(Some(90), Some(10));
        total.merge(&Usage::new(5, 1).with_cache_approximate(Some(30), Some(20)));
        assert_eq!(total.cache_hit_rate, Some(120.0 / 150.0));
        assert!(total.cache_hit_rate_approximate);

        // Sticky in the other fold order too.
        let mut reversed = Usage::new(5, 1).with_cache_approximate(Some(30), Some(20));
        reversed.merge(&Usage::new(10, 1).with_cache(Some(90), Some(10)));
        assert!(reversed.cache_hit_rate_approximate);

        // An approximate flag cannot survive an incomplete aggregate: the
        // rate is unknown, so "approximate" would misdescribe it.
        let mut unknown = Usage::new(10, 1).with_cache_approximate(Some(90), Some(10));
        unknown.merge(&Usage::new(5, 1).with_cache(Some(10), None));
        assert_eq!(unknown.cache_hit_rate, None);
        assert!(unknown.cache_hit_rate_approximate);
    }

    #[test]
    fn approximate_flag_serializes_for_the_renderer() {
        let approx = Usage::new(10, 1).with_cache_approximate(Some(90), Some(10));
        let json = serde_json::to_value(&approx).unwrap();
        assert_eq!(json["cache_hit_rate_approximate"], json!(true));

        // Absent when false, so old consumers see an unchanged shape.
        let real = Usage::new(10, 1).with_cache(Some(90), Some(10));
        let json = serde_json::to_value(&real).unwrap();
        assert!(json.get("cache_hit_rate_approximate").is_none());

        // Round-trips.
        let back: Usage = serde_json::from_value(json).unwrap();
        assert!(!back.cache_hit_rate_approximate);
    }
}
