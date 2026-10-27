//! Transport services: the listener and its graceful shutdown path.

pub mod serve;

pub use serve::{bind, AppHandle};
