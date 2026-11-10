//! Service layer: host wiring, isolation handles, loader and registry.

pub mod context;
pub mod directory;
pub mod hook_audit;
pub mod host;
pub mod isolation;
pub mod plugin;
pub mod provision;
pub mod registry;

pub use context::{HostHandles, PluginContext};
pub use directory::{scan, DirectoryPlugin, ScannedPlugin, MANIFEST_FILE};
pub use hook_audit::{HookAuditRecord, HookAuditSink, InMemoryHookAudit};
pub use host::{load_host, PluginHost};
pub use isolation::{
    ScopedConfig, ScopedCredentials, ScopedFiles, ScopedKv, ScopedLog, ScopedMetrics, ScopedNotify,
    ScopedTrace,
};
pub use plugin::{Plugin, Registration};
pub use provision::{CommandRunner, OsCommandRunner, ProvisionFlow, ProvisionSpec};
pub use registry::{
    LoadReport, MemoryPluginDiagnostics, PluginDiagnostic, PluginDiagnosticSink, PluginRegistry,
};
