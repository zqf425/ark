//! Service layer: the `SessionStore` implementation the runtime calls.
//!
//! Composition only: each service module owns one concern and the store facade
//! wires them together (design 5.1 api/service/domain/store layering).

pub mod child_service;
pub mod delegation;
pub mod memory;

pub use memory::MemoryFileStore;
pub mod reconcile;
pub mod store;
pub mod thread_ops;
pub mod writer_registry;

pub use child_service::ChildStore;
pub use delegation::{DelegateError, DelegationPolicy, MAX_DEPTH};
pub use reconcile::{reconcile_index, ReconcileReport};
pub use store::FileSessionStore;
pub use writer_registry::WriterRegistry;
