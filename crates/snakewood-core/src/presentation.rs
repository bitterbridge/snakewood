use crate::Direction;

/// A semantic unit of output. Transports (Stage 3) render these; the core never
/// emits formatted text or ANSI.
#[derive(Debug, Clone, PartialEq)]
pub enum PresentationNode {
    RoomName(String),
    RoomDescription(String),
    Exits(Vec<Direction>),
    Occupants(Vec<String>),
    Line(String),
    Denied(String),
    Prompt,
}
