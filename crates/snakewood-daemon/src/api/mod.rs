//! The structured command API: newline-delimited JSON in/out (the MCP bridge taps this).

pub mod handler;
pub mod protocol;

pub use handler::handle_api_request;
pub use protocol::{ApiRequest, ApiResponse};
