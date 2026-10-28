//! Model client implementations.
//!
//! [`mock`] is scriptable and emits chunk by chunk, which is what the agent loop
//! and the tests need. [`http`] is the real transport and owns no wire-format
//! knowledge: `provider::route` gives it a family, it just moves bytes.

pub mod catalog;
pub mod http;
pub mod mock;

pub use catalog::ModelCatalogClient;

pub use http::HttpModelClient;
pub use mock::{MockModelClient, Script, ScriptedCall};
