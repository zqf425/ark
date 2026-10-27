//! Service layer: the bus (single egress) and the client transports.

pub mod bus;
pub mod registry;
pub mod servers;
pub mod transport;

pub use bus::McpBus;
pub use registry::{CallContext, ExposureHandle, McpHandler, RegisteredTool, ToolRegistry};
pub use servers::{McpServerRegistry, McpToolAdapter};
pub use transport::{
    HttpTransportConfig, McpTransport, SseTransport, StdioTransport, StreamableHttpTransport,
    TransportKind,
};
