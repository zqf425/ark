//! Store configuration and hard resource limits (design 7.4.3).
//!
//! Limits live in a struct rather than as scattered constants so the runtime
//! can tune them from settings and so tests can shrink them (`fsync_on_append`
//! off, tiny live buffers) without touching production defaults.

use std::path::{Path, PathBuf};

/// Default bounded live channel per subscriber (design 7.4.3).
pub const DEFAULT_LIVE_CAPACITY: usize = 512;

/// Hard ceiling so a bad config cannot create an unbounded channel.
pub const MAX_LIVE_CAPACITY: usize = 4096;

/// Compaction keeps this many most-recent events when the caller does not say.
pub const DEFAULT_COMPACT_KEEP_LAST: usize = 2048;

/// Resource budgets for one store instance.
#[derive(Debug, Clone)]
pub struct StoreLimits {
    /// Per-subscriber bounded channel capacity.
    pub live_capacity: usize,
    /// `fsync` the event log after each append. Durability versus throughput.
    pub fsync_on_append: bool,
    /// Reject `subscribe` backlogs larger than this many events.
    pub max_backlog_events: usize,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            live_capacity: DEFAULT_LIVE_CAPACITY,
            fsync_on_append: true,
            max_backlog_events: 100_000,
        }
    }
}

impl StoreLimits {
    /// Clamp every field into its permitted range.
    pub fn sanitized(mut self) -> Self {
        self.live_capacity = self.live_capacity.clamp(1, MAX_LIVE_CAPACITY);
        self.max_backlog_events = self.max_backlog_events.clamp(1, 1_000_000);
        self
    }

    /// Limits tuned for tests: no fsync, small buffers.
    pub fn for_tests() -> Self {
        Self {
            live_capacity: 8,
            fsync_on_append: false,
            max_backlog_events: 10_000,
        }
    }
}

/// Where the store lives plus its budgets.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    data_dir: PathBuf,
    limits: StoreLimits,
}

impl StoreConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            limits: StoreLimits::default(),
        }
    }

    pub fn with_limits(mut self, limits: StoreLimits) -> Self {
        self.limits = limits.sanitized();
        self
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn limits(&self) -> &StoreLimits {
        &self.limits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_capacity_is_clamped_to_the_hard_ceiling() {
        let limits = StoreLimits {
            live_capacity: 1_000_000,
            ..StoreLimits::default()
        }
        .sanitized();
        assert_eq!(limits.live_capacity, MAX_LIVE_CAPACITY);

        let zero = StoreLimits {
            live_capacity: 0,
            ..StoreLimits::default()
        }
        .sanitized();
        assert_eq!(zero.live_capacity, 1);
    }

    #[test]
    fn config_keeps_the_data_dir_verbatim() {
        let config = StoreConfig::new("/tmp/ark-data").with_limits(StoreLimits::for_tests());
        assert_eq!(config.data_dir(), Path::new("/tmp/ark-data"));
        assert_eq!(config.limits().live_capacity, 8);
    }
}
