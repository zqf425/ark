//! Service layer: discovery, activation, catalog rendering and the runtime
//! facade. This is the only layer the HTTP routes talk to.

pub mod activation;
pub mod catalog;
pub mod discovery;
pub mod runtime;

pub use runtime::SkillRuntime;
