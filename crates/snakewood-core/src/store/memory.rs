use std::collections::BTreeMap;

use crate::store::{CommitId, StoreError, WorldStore};
use crate::{EntityId, Room, World};

/// An in-memory store for fast tests. "Commits" are recorded as snapshots so
/// behavior mirrors the git store closely enough for logic tests.
#[derive(Default)]
pub struct MemoryStore {
    rooms: BTreeMap<EntityId, Room>,
    commits: Vec<String>,
    next_commit: u64,
}

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }
}

impl WorldStore for MemoryStore {
    fn save_room(&mut self, room: &Room) -> Result<(), StoreError> {
        self.rooms.insert(room.id.clone(), room.clone());
        Ok(())
    }

    fn load_all(&self) -> Result<World, StoreError> {
        Ok(World {
            rooms: self.rooms.clone(),
        })
    }

    fn commit(&mut self, message: &str, _epoch_seconds: i64) -> Result<CommitId, StoreError> {
        self.commits.push(message.to_string());
        let id = CommitId(format!("mem-{}", self.next_commit));
        self.next_commit += 1;
        Ok(id)
    }

    fn commit_log(&self) -> Vec<String> {
        self.commits.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Direction;

    fn clearing() -> Room {
        let mut exits = BTreeMap::new();
        exits.insert(
            Direction::North,
            EntityId::new("snakewood/old-well").unwrap(),
        );
        Room {
            id: EntityId::new("snakewood/clearing").unwrap(),
            name: "Snakewood Clearing".to_string(),
            description: "A clearing.".to_string(),
            exits,
        }
    }

    #[test]
    fn saves_and_loads_a_room() {
        let mut store = MemoryStore::new();
        let room = clearing();
        store.save_room(&room).unwrap();
        let world = store.load_all().unwrap();
        assert_eq!(world.room(&room.id), Some(&room));
    }

    #[test]
    fn records_commit_messages_in_order() {
        let mut store = MemoryStore::new();
        store.commit("first", 1000).unwrap();
        store.commit("second", 2000).unwrap();
        assert_eq!(
            store.commit_log(),
            vec!["first".to_string(), "second".to_string()]
        );
    }
}
