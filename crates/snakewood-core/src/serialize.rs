use ron::ser::PrettyConfig;

use crate::Room;

fn pretty_config() -> PrettyConfig {
    // Deterministic, human-readable output. Defaults already sort nothing
    // random; our types use BTreeMap + fixed field order, so output is stable.
    PrettyConfig::default()
        .struct_names(true)
        .indentor("    ".to_string())
}

/// Serialize a room to canonical pretty RON.
pub fn room_to_ron(room: &Room) -> String {
    ron::ser::to_string_pretty(room, pretty_config())
        .expect("Room serialization is infallible for our field types")
}

/// Parse a room from RON text.
pub fn room_from_ron(s: &str) -> Result<Room, ron::error::SpannedError> {
    ron::from_str(s)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{Direction, EntityId};

    fn clearing() -> Room {
        let mut exits = BTreeMap::new();
        exits.insert(Direction::North, EntityId::new("snakewood/old-well").unwrap());
        exits.insert(Direction::Down, EntityId::new("snakewood/root-cellar").unwrap());
        Room {
            id: EntityId::new("snakewood/clearing").unwrap(),
            name: "Snakewood Clearing".to_string(),
            description: "Gnarled snakewood trees ring a clearing.".to_string(),
            exits,
        }
    }

    #[test]
    fn round_trips_losslessly() {
        let room = clearing();
        let text = room_to_ron(&room);
        let parsed = room_from_ron(&text).unwrap();
        assert_eq!(parsed, room);
    }

    #[test]
    fn serialization_is_deterministic() {
        let room = clearing();
        assert_eq!(room_to_ron(&room), room_to_ron(&room));
    }

    #[test]
    fn exit_keys_are_sorted_by_direction_order() {
        // Down < North in declaration order, so Down must appear before North
        // regardless of insertion order.
        let room = clearing();
        let text = room_to_ron(&room);
        let down_pos = text.find("Down").expect("Down present");
        let north_pos = text.find("North").expect("North present");
        assert!(down_pos < north_pos, "exits must serialize in Direction order:\n{text}");
    }
}
