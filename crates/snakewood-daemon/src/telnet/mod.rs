//! The telnet transport: translate a line-oriented text stream to/from the fabric.

pub mod parse;
pub mod provision;
pub mod render;

pub use parse::{is_quit, parse};
pub use provision::{despawn_player, spawn_player};
pub use render::render;
