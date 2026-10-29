//! ark-common: cross-module pure primitives.
//!
//! Per design §5.1 this crate mirrors the `qdc-common` layer: errors, ids,
//! stable serialization helpers and byte-accurate text utilities. It owns no
//! I/O and no async runtime, so every module here is unit-testable in isolation.

pub mod encoding;
pub mod error;
pub mod ids;
pub mod path;
pub mod text;
pub mod usage;

pub use encoding::{
    decode, decode_to_string, read_json_file, read_json_file_opt, read_text, read_text_file,
    read_text_opt, strip_bom_bytes, strip_bom_str, utf8_bytes_no_bom, write_json_file, write_text,
    write_text_atomic, DecodedText, TextEncoding, UTF8_BOM_BYTES, UTF8_BOM_CHAR,
};
pub use error::{ArkError, ArkErrorCode, ArkErrorPayload, Result};
pub use ids::{AttachmentId, ChildId, ItemId, RunId, ThreadId, TurnId};
pub use usage::Usage;
