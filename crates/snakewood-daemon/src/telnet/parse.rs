use snakewood_core::{Direction, EntityId, Intent};

/// Parse one line of player input into an intent for `actor`.
/// Returns `None` for empty input or an unrecognized command.
pub fn parse(line: &str, actor: &EntityId) -> Option<Intent> {
    let word = line.trim().to_ascii_lowercase();
    let direction = match word.as_str() {
        "n" | "north" => Some(Direction::North),
        "s" | "south" => Some(Direction::South),
        "e" | "east" => Some(Direction::East),
        "w" | "west" => Some(Direction::West),
        "u" | "up" => Some(Direction::Up),
        "d" | "down" => Some(Direction::Down),
        _ => None,
    };
    if let Some(direction) = direction {
        return Some(Intent::Move {
            actor: actor.clone(),
            direction,
        });
    }
    match word.as_str() {
        "look" | "l" => Some(Intent::Look {
            actor: actor.clone(),
        }),
        _ => None,
    }
}

/// Whether a line is a request to disconnect (a transport concern, not an Intent).
pub fn is_quit(line: &str) -> bool {
    matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "quit" | "q" | "exit"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> EntityId {
        EntityId::new("player/anon-0").unwrap()
    }

    #[test]
    fn parses_movement_words_and_abbreviations() {
        assert_eq!(
            parse("north", &actor()),
            Some(Intent::Move {
                actor: actor(),
                direction: Direction::North
            })
        );
        assert_eq!(
            parse("N", &actor()),
            Some(Intent::Move {
                actor: actor(),
                direction: Direction::North
            })
        );
        assert_eq!(
            parse("  d  ", &actor()),
            Some(Intent::Move {
                actor: actor(),
                direction: Direction::Down
            })
        );
    }

    #[test]
    fn parses_look() {
        assert_eq!(
            parse("look", &actor()),
            Some(Intent::Look { actor: actor() })
        );
        assert_eq!(parse("l", &actor()), Some(Intent::Look { actor: actor() }));
    }

    #[test]
    fn empty_and_unknown_are_none() {
        assert_eq!(parse("", &actor()), None);
        assert_eq!(parse("   ", &actor()), None);
        assert_eq!(parse("fluffernuts", &actor()), None);
    }

    #[test]
    fn quit_detection() {
        assert!(is_quit("quit"));
        assert!(is_quit(" Q "));
        assert!(is_quit("exit"));
        assert!(!is_quit("look"));
    }
}
