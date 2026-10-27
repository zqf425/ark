//! Usage metering (design 7.11, 6.4).
//!
//! The recorder is the only place that aggregates usage counters. It keeps the
//! distinction between "unknown" and "zero": a provider that reports no cache
//! numbers must not be turned into a 0% hit rate.

use ark_common::Usage;
use std::sync::Mutex;

/// Snapshot of everything the recorder has seen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageTotals {
    /// Aggregated token counters.
    pub usage: Usage,
    /// Completed turns.
    pub turns: u64,
    /// Individual usage samples folded in.
    pub samples: u64,
}

/// Thread-safe usage accumulator.
///
/// A single instance lives in the runtime and is shared by the agent loop, so
/// `GET /v1/usage` reads one consistent total instead of summing per-call state.
#[derive(Debug, Default)]
pub struct UsageRecorder {
    totals: Mutex<UsageTotals>,
}

impl UsageRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one provider usage sample into the running total.
    pub fn record(&self, usage: &Usage) {
        if let Ok(mut totals) = self.totals.lock() {
            totals.usage.merge(usage);
            totals.samples += 1;
        }
    }

    /// Record a completed turn plus the usage it consumed.
    pub fn record_turn(&self, usage: &Usage) {
        if let Ok(mut totals) = self.totals.lock() {
            totals.usage.merge(usage);
            totals.turns += 1;
        }
    }

    /// Current totals. Cheap enough to call per request.
    pub fn totals(&self) -> UsageTotals {
        self.totals
            .lock()
            .map(|totals| totals.clone())
            .unwrap_or_default()
    }

    /// Just the aggregated counters.
    pub fn usage(&self) -> Usage {
        self.totals().usage
    }

    /// Turn count reported by `GET /v1/usage`.
    pub fn turns(&self) -> u64 {
        self.totals().turns
    }

    /// Reset after a settings change that should not inherit old totals.
    pub fn reset(&self) {
        if let Ok(mut totals) = self.totals.lock() {
            *totals = UsageTotals::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_accumulate_tokens() {
        let recorder = UsageRecorder::new();
        recorder.record(&Usage::new(10, 5));
        recorder.record(&Usage::new(20, 5));
        let totals = recorder.totals();
        assert_eq!(totals.usage.prompt_tokens, 30);
        assert_eq!(totals.usage.completion_tokens, 10);
        assert_eq!(totals.usage.total_tokens, 40);
        assert_eq!(totals.samples, 2);
    }

    #[test]
    fn turns_are_counted_separately_from_samples() {
        let recorder = UsageRecorder::new();
        recorder.record_turn(&Usage::new(10, 5));
        recorder.record(&Usage::new(1, 1));
        assert_eq!(recorder.turns(), 1);
        assert_eq!(recorder.totals().samples, 1);
    }

    #[test]
    fn unknown_cache_counters_stay_unknown() {
        let recorder = UsageRecorder::new();
        recorder.record(&Usage::new(10, 5));
        assert_eq!(recorder.usage().cache_hit_rate, None);

        recorder.record(&Usage::new(10, 5).with_cache(Some(90), Some(10)));
        let usage = recorder.usage();
        assert_eq!(usage.cache_hit_tokens, Some(90));
        assert_eq!(usage.cache_miss_tokens, Some(10));
        assert_eq!(usage.cache_hit_rate, None);
    }

    #[test]
    fn reset_clears_everything() {
        let recorder = UsageRecorder::new();
        recorder.record_turn(&Usage::new(10, 5));
        recorder.reset();
        assert_eq!(recorder.turns(), 0);
        assert_eq!(recorder.usage().total_tokens, 0);
    }

    #[test]
    fn concurrent_records_do_not_lose_samples() {
        let recorder = std::sync::Arc::new(UsageRecorder::new());
        let mut handles = Vec::new();
        for _ in 0..8 {
            let recorder = recorder.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    recorder.record(&Usage::new(1, 1));
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(recorder.totals().samples, 800);
        assert_eq!(recorder.usage().total_tokens, 1600);
    }
}
