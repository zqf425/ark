use serde::{Deserialize, Serialize};

/// The one and only usage shape (design §6.4).
///
/// Provider-native fields are preserved verbatim. `cache_hit_rate` is only
/// derived when both hit and miss counters exist: never faked from
/// `hit / prompt_tokens`, because that silently overstates efficiency.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hit_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_miss_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hit_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
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
    pub fn merge(&mut self, other: &Usage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
        self.cache_hit_tokens = sum_opt(self.cache_hit_tokens, other.cache_hit_tokens);
        self.cache_miss_tokens = sum_opt(self.cache_miss_tokens, other.cache_miss_tokens);
        self.cache_hit_rate = Usage::rate(self.cache_hit_tokens, self.cache_miss_tokens);
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
}
