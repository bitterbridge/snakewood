use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Direction, EntityId};

/// A single authored place in the world. Exits are the ergonomic "sugar" form:
/// a direction mapping straight to a destination room id.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Room {
    pub id: EntityId,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub exits: BTreeMap<Direction, EntityId>,
}

/// The in-memory aggregate of all authored rooms.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct World {
    pub rooms: BTreeMap<EntityId, Room>,
}

impl World {
    pub fn insert_room(&mut self, room: Room) {
        self.rooms.insert(room.id.clone(), room);
    }

    pub fn room(&self, id: &EntityId) -> Option<&Room> {
        self.rooms.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clearing() -> Room {
        let mut exits = BTreeMap::new();
        exits.insert(Direction::North, EntityId::new("snakewood/old-well").unwrap());
        Room {
            id: EntityId::new("snakewood/clearing").unwrap(),
            name: "Snakewood Clearing".to_string(),
            description: "Gnarled snakewood trees ring a clearing of trampled grass.".to_string(),
            exits,
        }
    }

    #[test]
    fn insert_and_fetch_room() {
        let mut world = World::default();
        let room = clearing();
        let id = room.id.clone();
        world.insert_room(room.clone());
        assert_eq!(world.room(&id), Some(&room));
    }

    #[test]
    fn missing_room_is_none() {
        let world = World::default();
        let id = EntityId::new("snakewood/nowhere").unwrap();
        assert_eq!(world.room(&id), None);
    }
}
