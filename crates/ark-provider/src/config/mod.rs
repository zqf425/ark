//! Fixed knobs for the model provider table.
//!
//! The file name, the validation limits and the probe budget live here so no
//! route handler or adapter hard-codes them a second time.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Provider table file name inside the runtime data directory.
pub const PROVIDERS_FILE: &str = "providers.json";

/// Maximum provider id length, matching `^[A-Za-z0-9._-]{1,64}$`.
pub const ID_MAX_LEN: usize = 64;

/// Characters shown before the mask in an api-key hint.
pub const KEY_HINT_PREFIX: usize = 4;

/// Characters shown after the mask in an api-key hint.
pub const KEY_HINT_SUFFIX: usize = 2;

/// Keys shorter than this are masked completely: a 7-character secret would be
/// almost fully revealed by a prefix/suffix hint.
pub const KEY_HINT_MIN_LEN: usize = 8;

/// Replacement for the hidden part of a secret.
pub const KEY_HINT_MASK: &str = "***";

/// Wall-clock budget for one connectivity probe.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// `max_tokens` of the probe request: a handful of tokens is enough to prove
/// reachability without paying for a real completion.
pub const PROBE_MAX_TOKENS: u32 = 8;

/// Prompt of the probe request.
pub const PROBE_PROMPT: &str = "ping";

/// Response body bytes kept in a probe message.
pub const PROBE_BODY_LIMIT: usize = 300;

/// Status reported for a probe that succeeded.
///
/// `ark_llm::HttpModelClient` publishes parsed deltas, not the raw status line,
/// so a successful probe has no status to forward and reports 200.
pub const PROBE_OK_STATUS: u16 = 200;

/// `<data_dir>/providers.json`.
pub fn providers_path(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join(PROVIDERS_FILE)
}
