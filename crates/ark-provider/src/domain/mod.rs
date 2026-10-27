//! Data-only rules: what a valid provider entry is, how a key is masked, and
//! the on-disk document shape.
//!
//! Nothing here touches I/O or a lock, so every rule is unit-testable in
//! isolation and the service layer stays about orchestration.

pub mod doc;
pub mod mask;
pub mod validate;

pub use doc::ProvidersDocument;
pub use mask::{api_key_hint, mask_api_key, redact_url, scrub_secret};
pub use validate::{
    normalize_entry, validate_base_url, validate_endpoint_format, validate_id, validate_max_tokens,
    validate_model, validate_temperature,
};
