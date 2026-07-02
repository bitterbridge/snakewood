use std::collections::BTreeMap;

use crate::fabric::Rule;
use crate::{EntityId, Mob, World};

/// The fabric's operating context: authored rooms (`world`) plus live `mobs`
/// and global `rules`. Co-presence is derived on demand.
#[derive(Debug, Clone, Default)]
pub struct Realm {
    pub world: World,
    pub mobs: BTreeMap<EntityId, Mob>,
    pub rules: Vec<Rule>,
}

impl Realm {
    pub fn new(world: World) -> Realm {
        Realm {
            world,
            mobs: BTreeMap::new(),
            rules: Vec::new(),
        }
    }

    pub fn insert_mob(&mut self, mob: Mob) {
        self.mobs.insert(mob.id.clone(), mob);
    }

    pub fn mob(&self, id: &EntityId) -> Option<&Mob> {
        self.mobs.get(id)
    }

    pub fn mob_mut(&mut self, id: &EntityId) -> Option<&mut Mob> {
        self.mobs.get_mut(id)
    }

    pub fn mob_location(&self, id: &EntityId) -> Option<&EntityId> {
        self.mobs.get(id).map(|m| &m.location)
    }

    /// All mobs currently in `room`, sorted by id (deterministic). Derived each
    /// call — there is no stored subscription list to go stale.
    pub fn mobs_in_room(&self, room: &EntityId) -> Vec<&Mob> {
        self.mobs.values().filter(|m| &m.location == room).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{Flag, World};

    fn mob_at(id: &str, room: &str) -> Mob {
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        Mob {
            id: EntityId::new(id).unwrap(),
            name: id.to_string(),
            location: EntityId::new(room).unwrap(),
            flags,
            responders: Vec::new(),
        }
    }

    #[test]
    fn mobs_in_room_returns_only_co_located_sorted() {
        let mut realm = Realm::new(World::default());
        realm.insert_mob(mob_at("snakewood/mob/b#1", "snakewood/clearing"));
        realm.insert_mob(mob_at("snakewood/mob/a#1", "snakewood/clearing"));
        realm.insert_mob(mob_at("snakewood/mob/c#1", "snakewood/old-well"));

        let clearing = EntityId::new("snakewood/clearing").unwrap();
        let here: Vec<&str> = realm.mobs_in_room(&clearing).iter().map(|m| m.id.as_str()).collect();
        // BTreeMap iteration is sorted, so a before b; c is elsewhere.
        assert_eq!(here, vec!["snakewood/mob/a#1", "snakewood/mob/b#1"]);
    }

    #[test]
    fn mob_location_tracks_current_room() {
        let mut realm = Realm::new(World::default());
        realm.insert_mob(mob_at("snakewood/mob/a#1", "snakewood/clearing"));
        let a = EntityId::new("snakewood/mob/a#1").unwrap();
        assert_eq!(realm.mob_location(&a).map(|r| r.as_str()), Some("snakewood/clearing"));
    }
}
