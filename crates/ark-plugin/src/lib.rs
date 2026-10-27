//! ark-plugin: Plugin registry, manifest validation, scaffold surface (design 7.7).
//!
//! Layer folders follow the QDC mapping in design 5.1:
//!
//! * [`domain`] - pure rules: manifest/version, host-issued identity, startup
//!   topology, file-root containment. No I/O, so every invariant is testable.
//! * [`service`] - the loader ([`PluginRegistry`]), the host-built
//!   [`PluginContext`] with its scoped platform handles, and the revocable
//!   [`Registration`].
//!
//! ## Two failure policies, deliberately different
//!
//! * **Declaration errors** - dependency cycle, missing service, major mismatch,
//!   duplicate id - abort the whole load *before* any plugin code runs. There is
//!   no runtime fallback that could hide a broken manifest (design 7.5.4).
//! * **Load errors** - a plugin whose `register` fails - only skip that plugin
//!   and its dependents, recording `plugin_load_failed`. Unrelated plugins still
//!   start, so one broken plugin cannot poison the runtime (design 7.7.3).
//!
//! ## Isolation is not optional
//!
//! [`PluginIdentity`] has private fields and no public constructor: the host
//! issues it from the manifest it just read. Every platform handle
//! (`cfg`, `kv`, `files`, `cred`, `log`, `trace`, `metrics`, `notify`) is built
//! by the host with that id baked in and exposes no owner argument, so a plugin
//! cannot address another plugin's namespace, file root or secrets. There is no
//! switch to turn this off.

#![deny(clippy::all)]

pub mod domain;
pub mod service;

pub use domain::{
    plan_skips, resolve_order, resolve_within_root, sanitize_relative, Grant, HttpSection,
    McpSection, PluginId, PluginIdentity, PluginKind, PluginManifest, TopologyErrorKind, Version,
    VersionReq,
};
pub use service::{
    HostHandles, LoadReport, MemoryPluginDiagnostics, Plugin, PluginContext, PluginDiagnostic,
    PluginDiagnosticSink, PluginRegistry, Registration, ScopedConfig, ScopedCredentials,
    ScopedFiles, ScopedKv, ScopedLog, ScopedMetrics, ScopedNotify, ScopedTrace,
};
