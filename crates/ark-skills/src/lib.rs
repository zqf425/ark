//! ark-skills: skill discovery, manifest parsing, budgets and activation
//! (design 7.14).
//!
//! Layer folders follow the QDC mapping in design 5.1:
//!
//! * `config` - [`SkillsConfig`] and path helpers (root expansion, defaults).
//! * `domain` - the [`LoadedSkill`] model and the `SKILL.md` front matter reader.
//! * `service` - filesystem discovery, trigger activation, catalog rendering and
//!   the [`SkillRuntime`] facade.
//!
//! The runtime keeps a scan snapshot behind a lock, so it is `Send + Sync` and
//! can be shared as an `Arc` by `ark-server`. Every failure while scanning is
//! downgraded to a warning or a validation error: a broken skill directory must
//! never take the process down.

#![deny(clippy::all)]

pub mod config;
pub mod domain;
pub mod service;

pub use config::{
    expand_home, home_dir, SkillsConfig, DEFAULT_ACTIVE_LIMIT, DEFAULT_CATALOG_BUDGET_BYTES,
    DEFAULT_INSTRUCTION_BUDGET_BYTES,
};
pub use domain::skill::{slug, LoadedSkill};
pub use service::catalog::render_catalog;
pub use service::discovery::ScanOutcome;
pub use service::runtime::SkillRuntime;
