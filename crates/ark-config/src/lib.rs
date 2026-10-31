//! ark-config: Config loading, ReloadClass validation and hot-apply (design §7.15).
//!
//! Layer folders follow the QDC mapping in design §5.1:
//!
//! ```text
//! domain/   data only: the snapshot, the validation rules, the class table
//! service/  ConfigStore (atomic swap) + JSON document persistence
//! ```
//!
//! Every field declares its `ReloadClass` in [`domain::classify`], and
//! [`service::ConfigStore::apply`] refuses a `Restart` field as a whole batch:
//! there is no partial application and no silent ignore.

#![deny(clippy::all)]

pub mod domain;
pub mod service;

pub use domain::{
    diagnose, relisten_fields, reload_class, restart_fields, tuning_diff, ConfigDiagnostics,
    RuntimeConfig, TuningConfig, DEFAULT_PORT, FIELD_CLASSES, LOG_LEVELS, TUNING_FIELDS,
};
pub use service::{
    classes_of, config_path, load_or_default, load_report, save, ConfigSnapshot, ConfigStore,
    LoadedConfig,
};
