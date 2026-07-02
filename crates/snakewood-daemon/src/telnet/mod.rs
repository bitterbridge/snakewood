//! The telnet transport: translate a line-oriented text stream to/from the fabric.

pub mod parse;

pub use parse::{is_quit, parse};
