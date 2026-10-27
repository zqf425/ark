//! Configuration and hard resource limits for the store.

pub mod store_config;

pub use store_config::{
    StoreConfig, StoreLimits, DEFAULT_COMPACT_KEEP_LAST, DEFAULT_LIVE_CAPACITY, MAX_LIVE_CAPACITY,
};
