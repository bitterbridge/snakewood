use serde::{Deserialize, Serialize};

use crate::Direction;

/// A semantic unit of output. Transports render these (telnet) or pass them as
/// structured data (the command API); the core never emits formatted text.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum PresentationNode {
    RoomName(String),
    RoomDescription(String),
    Exits(Vec<Direction>),
    Occupants(Vec<String>),
    Line(String),
    Denied(String),
    Prompt,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_node_round_trips_via_serde() {
        let node = PresentationNode::Exits(vec![Direction::North, Direction::Down]);
        let text = ron::ser::to_string(&node).unwrap();
        let back: PresentationNode = ron::from_str(&text).unwrap();
        assert_eq!(back, node);

        let line = PresentationNode::Line("hello".to_string());
        let back2: PresentationNode = ron::from_str(&ron::ser::to_string(&line).unwrap()).unwrap();
        assert_eq!(back2, line);
    }
}
