//! The structured command API: newline-delimited JSON in/out (the MCP bridge taps this).

pub mod protocol;

pub use protocol::{ApiRequest, ApiResponse};
