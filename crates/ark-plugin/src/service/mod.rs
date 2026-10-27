//! Service layer: host wiring, isolation handles, loader and registry.

pub mod context;
pub mod isolation;
pub mod plugin;
pub mod registry;

pub use context::{HostHandles, PluginContext};
pub use isolation::{
    ScopedConfig, ScopedCredentials, ScopedFiles, ScopedKv, ScopedLog, ScopedMetrics, ScopedNotify,
    ScopedTrace,
};
pub use plugin::{Plugin, Registration};
pub use registry::{
    LoadReport, MemoryPluginDiagnostics, PluginDiagnostic, PluginDiagnosticSink, PluginRegistry,
};
