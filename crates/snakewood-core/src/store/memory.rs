use std::collections::BTreeMap;

use crate::store::{CommitId, StoreError, WorldStore};
use crate::{EntityId, Mob, Operator, Room, Rule, World};

/// An in-memory store for fast tests. "Commits" are recorded as snapshots so
/// behavior mirrors the git store closely enough for logic tests.
#[derive(Default)]
pub struct MemoryStore {
    rooms: BTreeMap<EntityId, Room>,
    mobs: BTreeMap<EntityId, Mob>,
    rules: Vec<Rule>,
    operators: Vec<Operator>,
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

    fn save_mob(&mut self, mob: &Mob) -> Result<(), StoreError> {
        self.mobs.insert(mob.id.clone(), mob.clone());
        Ok(())
    }

    fn remove_mob(&mut self, id: &EntityId) -> Result<(), StoreError> {
        self.mobs.remove(id);
        Ok(())
    }

    fn load_mobs(&self) -> Result<Vec<Mob>, StoreError> {
        Ok(self.mobs.values().cloned().collect())
    }

    fn save_rules(&mut self, rules: &[Rule]) -> Result<(), StoreError> {
        self.rules = rules.to_vec();
        Ok(())
    }

    fn load_rules(&self) -> Result<Vec<Rule>, StoreError> {
        Ok(self.rules.clone())
    }

    fn save_operators(&mut self, operators: &[Operator]) -> Result<(), StoreError> {
        self.operators = operators.to_vec();
        Ok(())
    }

    fn load_operators(&self) -> Result<Vec<Operator>, StoreError> {
        Ok(self.operators.clone())
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

    #[test]
    fn saves_loads_and_removes_mobs() {
        use crate::{Flag, Mob};
        use std::collections::BTreeSet;
        let mut store = MemoryStore::new();
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        let mob = Mob {
            id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
            name: "a goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        };
        store.save_mob(&mob).unwrap();
        assert_eq!(store.load_mobs().unwrap(), vec![mob.clone()]);
        store.remove_mob(&mob.id).unwrap();
        assert!(store.load_mobs().unwrap().is_empty());
        // removing an absent mob is a no-op
        store.remove_mob(&mob.id).unwrap();
    }

    #[test]
    fn load_realm_composes_rooms_mobs_rules() {
        use crate::Mob;
        use std::collections::BTreeSet;
        let mut store = MemoryStore::new();
        store.save_room(&clearing()).unwrap();
        store
            .save_mob(&Mob {
                id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
                name: "a goblin".to_string(),
                location: EntityId::new("snakewood/clearing").unwrap(),
                flags: BTreeSet::new(),
                responders: Vec::new(),
            })
            .unwrap();
        let realm = store.load_realm().unwrap();
        assert!(realm
            .world
            .room(&EntityId::new("snakewood/clearing").unwrap())
            .is_some());
        assert!(realm
            .mob(&EntityId::new("snakewood/mob/goblin#1").unwrap())
            .is_some());
        // no_exit_message defaulted via Realm::new
        assert_eq!(realm.no_exit_message, "You see no exit in that direction.");
    }
}
