//! Configuration services: the one mutation point plus file IO.
//!
//! `ConfigStore` owns the atomic snapshot; `file_config` loads and persists the
//! JSON document so a restart sees what the last hot patch produced.

pub mod config_store;
pub mod file_config;

pub use config_store::{classes_of, ConfigSnapshot, ConfigStore};
pub use file_config::{config_path, load_or_default, save};
