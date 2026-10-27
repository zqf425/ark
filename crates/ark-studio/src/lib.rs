//! ark-studio: UI automation domain: case IR, RunBundle, gates (design §7.8).
//!
//! Layer folders follow the QDC mapping in design §5.1 (api/service/domain/
//! store/config/exec). Only the public surface is re-exported here.

#![deny(clippy::all)]

// Implementation lands incrementally; the crate must stay compiling so the
// dependency-direction lint can run on every step.
