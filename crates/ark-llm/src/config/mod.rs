//! Provider configuration. `EndpointFormat` is a type, not a string branch: the
//! router matches on it once (design §7.2) and each provider module receives a
//! fully resolved URL.

pub mod endpoint;
pub mod url;

pub use endpoint::{
    EndpointFormat, ProviderConfig, COMPACTION_TRIGGER_RATIO, DEFAULT_CONTEXT_WINDOW_TOKENS,
};
pub use url::{anthropic_messages_url, model_catalog_url, openai_chat_url, openai_responses_url};
