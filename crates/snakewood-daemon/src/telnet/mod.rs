//! The telnet transport: translate a line-oriented text stream to/from the fabric.

pub mod parse;
pub mod render;

pub use parse::{is_quit, parse};
pub use render::render;
