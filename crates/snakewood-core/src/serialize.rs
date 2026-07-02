use ron::ser::PrettyConfig;

use crate::Room;

fn pretty_config() -> PrettyConfig {
    // Deterministic, human-readable output. Defaults already sort nothing
    // random; our types use BTreeMap + fixed field order, so output is stable.
    PrettyConfig::default()
        .struct_names(true)
        .indentor("    ".to_string())
}

/// Serialize any value to canonical pretty RON.
pub fn to_ron<T: serde::Serialize>(value: &T) -> String {
    ron::ser::to_string_pretty(value, pretty_config())
        .expect("serialization is infallible for our field types")
}

/// Parse any value from RON text.
pub fn from_ron<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, ron::error::SpannedError> {
    ron::from_str(s)
}

/// Serialize a room to canonical pretty RON.
pub fn room_to_ron(room: &Room) -> String {
    to_ron(room)
}

/// Parse a room from RON text.
pub fn room_from_ron(s: &str) -> Result<Room, ron::error::SpannedError> {
    from_ron(s)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{Direction, EntityId};

    fn clearing() -> Room {
        let mut exits = BTreeMap::new();
        exits.insert(
            Direction::North,
            EntityId::new("snakewood/old-well").unwrap(),
        );
        exits.insert(
            Direction::Down,
            EntityId::new("snakewood/root-cellar").unwrap(),
        );
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
        // North is declared before Down in the Direction enum, so North must
        // appear before Down regardless of insertion order.
        let room = clearing();
        let text = room_to_ron(&room);
        let north_pos = text.find("North").expect("North present");
        let down_pos = text.find("Down").expect("Down present");
        assert!(
            north_pos < down_pos,
            "exits must serialize in Direction declaration order (North before Down):\n{text}"
        );
    }

    #[test]
    fn generic_ron_round_trips_a_vec() {
        let v: Vec<u32> = vec![3, 1, 2];
        let text = to_ron(&v);
        let back: Vec<u32> = from_ron(&text).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn room_helpers_match_generic() {
        let room = clearing();
        assert_eq!(room_to_ron(&room), to_ron(&room));
    }
}
