//! Pure workspace rules (design 7.9).
//!
//! Nothing here performs I/O: home expansion, relative resolution, status
//! skeletons and the parsing of git output are plain data transforms. That is
//! what lets the edge cases (`~`, empty input, detached `HEAD`) be unit tests
//! instead of machine-dependent integration tests.
//!
//! The status contract keeps one deliberate asymmetry: a field is `None` when
//! git *could not tell us* and `Some(false)` only when git answered. Guessing
//! "not dirty" from a failed command is exactly the bug this module avoids.

pub mod paths;
pub mod status;

pub use paths::{expand_home, home_dir, resolve_candidate};
pub use status::{base_status, clean_branch, clean_sha, count_status_lines, empty_status};
