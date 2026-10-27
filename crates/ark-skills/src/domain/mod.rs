//! Domain model for skill packages.
//!
//! Parsing and normalization live here; filesystem walking and turn resolution
//! live in `service`. Nothing in this module touches the network or an async
//! runtime.

pub mod frontmatter;
pub mod skill;

pub use frontmatter::FrontMatter;
pub use skill::{path_display, slug, LoadedSkill};
