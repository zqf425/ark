//! Service layer: the bus (single egress) and the client transports.

pub mod bus;
pub mod host_tool;
pub mod registry;
pub mod servers;
pub mod transport;

pub use bus::{McpBus, HOST_CALLER};
pub use host_tool::PluginTool;
pub use registry::{CallContext, ExposureHandle, McpHandler, RegisteredTool, ToolRegistry};
pub use servers::{McpServerRegistry, McpToolAdapter};
pub use transport::{
    HttpTransportConfig, McpTransport, SseTransport, StdioTransport, StreamableHttpTransport,
    TransportKind,
};
