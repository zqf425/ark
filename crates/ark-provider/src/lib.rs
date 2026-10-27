//! ark-provider: model provider table — CRUD, validation, masking and a
//! connectivity probe (design §7.2).
//!
//! # Layout
//!
//! ```text
//! config/   fixed knobs: file name, hint widths, probe budget
//! domain/   data-only rules: entry validation, key masking, the document
//! service/  ProviderStore, persistence, entry-to-client mapping, probe
//! ```
//!
//! # Why the split
//!
//! Validation and masking are pure functions, so every rule that protects a
//! secret is unit-tested without a filesystem or a network. The store is the
//! only mutation point: it validates, persists, and then swaps the in-memory
//! snapshot, which is what makes a hot reload visible to the very next
//! `resolve()` call.
//!
//! `api_key` is an input only. Read paths go through [`ProviderView`], which
//! reports `has_api_key` plus a masked hint; no log line or error message this
//! crate produces contains a plaintext secret.
//!
//! [`ProviderView`]: ark_contract::ProviderView

#![deny(clippy::all)]

pub mod config;
pub mod domain;
pub mod service;

pub use config::{
    providers_path, ID_MAX_LEN, KEY_HINT_MASK, KEY_HINT_MIN_LEN, KEY_HINT_PREFIX, KEY_HINT_SUFFIX,
    PROBE_BODY_LIMIT, PROBE_MAX_TOKENS, PROBE_PROMPT, PROBE_TIMEOUT, PROVIDERS_FILE,
};
pub use domain::{
    api_key_hint, mask_api_key, normalize_entry, redact_url, validate_base_url, validate_id,
    validate_model, validate_temperature, ProvidersDocument,
};
pub use service::{config_from_entry, DocumentLoad, ProviderStore};
