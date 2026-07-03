//! The MCP bridge: hand-rolled JSON-RPC over stdio, forwarding tool calls to the
//! daemon's command API. Synchronous; used by the `snakewood-mcp` binary.

pub mod protocol;
pub mod tools;

pub use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use tools::{response_to_text, tool_call_to_request, tool_definitions};
