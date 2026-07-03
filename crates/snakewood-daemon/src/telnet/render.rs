use snakewood_core::{Direction, PresentationNode, Span};

fn direction_name(dir: &Direction) -> &'static str {
    match dir {
        Direction::North => "north",
        Direction::South => "south",
        Direction::East => "east",
        Direction::West => "west",
        Direction::Up => "up",
        Direction::Down => "down",
    }
}

fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

fn render_node(node: &PresentationNode) -> Option<String> {
    match node {
        PresentationNode::RoomName(s) => Some(s.clone()),
        PresentationNode::RoomDescription(spans) => Some(spans_text(spans)),
        PresentationNode::Exits(dirs) => {
            if dirs.is_empty() {
                Some("Exits: none".to_string())
            } else {
                let names: Vec<&str> = dirs.iter().map(direction_name).collect();
                Some(format!("Exits: {}", names.join(", ")))
            }
        }
        PresentationNode::Occupants(spans) => {
            if spans.is_empty() {
                None // don't render an empty "Also here:" line
            } else {
                let names: Vec<&str> = spans.iter().map(|s| s.text.as_str()).collect();
                Some(format!("Also here: {}", names.join(", ")))
            }
        }
        PresentationNode::Line(spans) => Some(spans_text(spans)),
        PresentationNode::Denied(spans) => Some(spans_text(spans)),
        PresentationNode::Prompt => Some(">".to_string()),
    }
}

/// Render a batch of presentation nodes to telnet wire text (CRLF line endings).
pub fn render(nodes: &[PresentationNode]) -> String {
    let mut out = String::new();
    for node in nodes {
        if let Some(line) = render_node(node) {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_room_view() {
        let nodes = vec![
            PresentationNode::RoomName("Snakewood Clearing".to_string()),
            PresentationNode::RoomDescription(snakewood_core::plain_text("A clearing.")),
            PresentationNode::Exits(vec![Direction::North, Direction::Down]),
            PresentationNode::Occupants(vec![Span::actor("a goblin")]),
        ];
        let text = render(&nodes);
        assert_eq!(
            text,
            "Snakewood Clearing\r\nA clearing.\r\nExits: north, down\r\nAlso here: a goblin\r\n"
        );
    }

    #[test]
    fn empty_occupants_line_is_omitted() {
        let nodes = vec![PresentationNode::Occupants(vec![])];
        assert_eq!(render(&nodes), "");
    }

    #[test]
    fn no_exits_says_none() {
        let nodes = vec![PresentationNode::Exits(vec![])];
        assert_eq!(render(&nodes), "Exits: none\r\n");
    }

    #[test]
    fn renders_denied_and_line() {
        let nodes = vec![
            PresentationNode::Denied(snakewood_core::plain_text(
                "You see no exit in that direction.",
            )),
            PresentationNode::Line(snakewood_core::plain_text(
                "The goblin blocks your way north.",
            )),
        ];
        assert_eq!(
            render(&nodes),
            "You see no exit in that direction.\r\nThe goblin blocks your way north.\r\n"
        );
    }
}
