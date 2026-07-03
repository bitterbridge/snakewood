//! The structured command API: newline-delimited JSON in/out (the MCP bridge taps this).

pub mod handler;
pub mod protocol;
pub mod server;

pub use handler::{build_drain_response, handle_api_request, ApiOutcome, ReplyShape};
pub use protocol::{ApiRequest, ApiResponse};
pub use server::serve_api;
