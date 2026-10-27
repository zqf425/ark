//! Provider configuration. `EndpointFormat` is a type, not a string branch: the
//! router matches on it once (design §7.2) and each provider module receives a
//! fully resolved URL.

pub mod endpoint;
pub mod url;

pub use endpoint::{EndpointFormat, ProviderConfig};
pub use url::{anthropic_messages_url, openai_chat_url, openai_responses_url};
