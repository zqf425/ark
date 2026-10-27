use serde::{Deserialize, Serialize};

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
    /// Set once any folded sample omitted a native cache counter.
    ///
    /// Not serialized: it is bookkeeping for `merge`, not a wire field. Without
    /// it, folding `{hit: 90, miss: unknown}` and `{hit: 10, miss: 10}` produced
    /// `100/(100+10)` — a definite rate assembled from a value the provider never
    /// sent (design §6.4, P1-8).
    #[serde(skip)]
    cache_counters_incomplete: bool,
}

impl Usage {
    pub fn new(prompt_tokens: u64, completion_tokens: u64) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
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
        self.cost_usd = sum_opt_f(self.cost_usd, other.cost_usd);
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
}
