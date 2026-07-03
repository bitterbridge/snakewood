use snakewood_core::{Direction, PresentationNode, Role, Span};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderStyle {
    Ansi,
    Plain,
}

const RESET: &str = "\x1b[0m";

fn role_sgr(role: Role) -> Option<&'static str> {
    match role {
        Role::Default => None,
        Role::Actor => Some("\x1b[36m"), // cyan
    }
}

/// Wrap `text` in `sgr` + reset when Ansi and a code is given; else return text.
fn styled(text: &str, sgr: Option<&str>, style: RenderStyle) -> String {
    match (style, sgr) {
        (RenderStyle::Ansi, Some(code)) => format!("{code}{text}{RESET}"),
        _ => text.to_string(),
    }
}

/// Render one span, applying its role colour in Ansi mode.
fn render_span(span: &Span, style: RenderStyle) -> String {
    styled(&span.text, role_sgr(span.role), style)
}

fn render_node(node: &PresentationNode, style: RenderStyle) -> Option<String> {
    match node {
        PresentationNode::RoomName(s) => Some(styled(s, Some("\x1b[1m"), style)), // bold
        PresentationNode::RoomDescription(spans) => {
            Some(spans.iter().map(|sp| render_span(sp, style)).collect())
        }
        PresentationNode::Exits(dirs) => {
            let body = if dirs.is_empty() {
                "Exits: none".to_string()
            } else {
                let names: Vec<&str> = dirs.iter().map(direction_name).collect();
                format!("Exits: {}", names.join(", "))
            };
            Some(styled(&body, Some("\x1b[2m"), style)) // dim
        }
        PresentationNode::Occupants(spans) => {
            if spans.is_empty() {
                None // don't render an empty "Also here:" line
            } else {
                let names: Vec<String> = spans.iter().map(|sp| render_span(sp, style)).collect();
                Some(format!("Also here: {}", names.join(", ")))
            }
        }
        PresentationNode::Line(spans) => {
            Some(spans.iter().map(|sp| render_span(sp, style)).collect())
        }
        PresentationNode::Denied(spans) => {
            // Denial base style is red. Spans are all Default in M2, so we wrap
            // the whole line rather than styling per span. NOTE(M3): if inline
            // roles (e.g. Actor) ever reach a Denied node, switch to per-span
            // rendering under a red base so those roles aren't collapsed here.
            let body: String = spans.iter().map(|sp| sp.text.as_str()).collect();
            Some(styled(&body, Some("\x1b[31m"), style)) // red
        }
        PresentationNode::Prompt => Some(styled(">", Some("\x1b[2m"), style)), // dim
    }
}

/// Render a batch of presentation nodes to telnet wire text (CRLF line endings).
pub fn render(nodes: &[PresentationNode], style: RenderStyle) -> String {
    let mut out = String::new();
    for node in nodes {
        if let Some(line) = render_node(node, style) {
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
        let text = render(&nodes, RenderStyle::Plain);
        assert_eq!(
            text,
            "Snakewood Clearing\r\nA clearing.\r\nExits: north, down\r\nAlso here: a goblin\r\n"
        );
    }

    #[test]
    fn empty_occupants_line_is_omitted() {
        let nodes = vec![PresentationNode::Occupants(vec![])];
        assert_eq!(render(&nodes, RenderStyle::Plain), "");
    }

    #[test]
    fn no_exits_says_none() {
        let nodes = vec![PresentationNode::Exits(vec![])];
        assert_eq!(render(&nodes, RenderStyle::Plain), "Exits: none\r\n");
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
            render(&nodes, RenderStyle::Plain),
            "You see no exit in that direction.\r\nThe goblin blocks your way north.\r\n"
        );
    }

    #[test]
    fn plain_mode_is_unstyled() {
        let nodes = vec![
            PresentationNode::RoomName("Snakewood Clearing".to_string()),
            PresentationNode::Occupants(vec![Span::actor("a goblin")]),
        ];
        let text = render(&nodes, RenderStyle::Plain);
        assert_eq!(text, "Snakewood Clearing\r\nAlso here: a goblin\r\n");
        assert!(
            !text.contains('\x1b'),
            "plain mode must emit no escape codes"
        );
    }

    #[test]
    fn ansi_mode_styles_roomname_bold_and_actor_cyan() {
        let nodes = vec![
            PresentationNode::RoomName("Snakewood Clearing".to_string()),
            PresentationNode::Occupants(vec![Span::actor("a goblin")]),
            PresentationNode::Denied(snakewood_core::plain_text("Nope.")),
        ];
        let text = render(&nodes, RenderStyle::Ansi);
        // RoomName is bold; the literal text survives between codes.
        assert!(text.contains("\x1b[1mSnakewood Clearing\x1b[0m"));
        // Actor span is cyan.
        assert!(text.contains("\x1b[36ma goblin\x1b[0m"));
        // Denied line is red.
        assert!(text.contains("\x1b[31mNope.\x1b[0m"));
        // Substring of the raw words still present (so e2e substring matches survive).
        assert!(text.contains("Snakewood Clearing"));
    }
}
