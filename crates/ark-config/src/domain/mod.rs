//! Pure configuration rules (design §7.15.1).
//!
//! Nothing here performs I/O: the snapshot, the validation table and the reload
//! classification are data, which is what makes "invalid patch changes nothing"
//! a unit test rather than an integration test.

pub mod classify;
pub mod diagnostics;
pub mod snapshot;

pub use classify::{
    relisten_fields, reload_class, restart_fields, tuning_diff, FIELD_CLASSES, TUNING_FIELDS,
};
pub use diagnostics::{diagnose, ConfigDiagnostics, CONFIG_KEYS, SUBAGENT_KEYS, TUNING_KEYS};
pub use snapshot::{RuntimeConfig, TuningConfig, LOG_LEVELS};

/// Default listen port for `arkd serve` (design §7.9).
pub const DEFAULT_PORT: u16 = 18790;
