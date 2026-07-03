use std::collections::BTreeMap;

use crate::fabric::Rule;
use crate::{EntityId, Mob, World};

/// The fabric's operating context: authored rooms (`world`) plus live `mobs`
/// and global `rules`. Co-presence is derived on demand.
#[derive(Debug, Clone)]
pub struct Realm {
    pub world: World,
    pub mobs: BTreeMap<EntityId, Mob>,
    pub rules: Vec<Rule>,
    /// Message shown when a movement intent resolves to no exit at all.
    /// Data, not hardcoded, so content can reword/localize it.
    pub no_exit_message: String,
    /// Declarative stream operators attached to this realm (authored data).
    pub operators: Vec<crate::Operator>,
    /// Message shown when a RateLimit operator drops an intent and the operator
    /// carries no explicit `deny` text. Data, not hardcoded.
    pub rate_limit_message: String,
}

impl Realm {
    pub fn new(world: World) -> Realm {
        Realm {
            world,
            mobs: BTreeMap::new(),
            rules: Vec::new(),
            no_exit_message: "You see no exit in that direction.".to_string(),
            operators: Vec::new(),
            rate_limit_message: "You can't do that yet.".to_string(),
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

impl Default for Realm {
    fn default() -> Realm {
        Realm::new(World::default())
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
        let here: Vec<&str> = realm
            .mobs_in_room(&clearing)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        // BTreeMap iteration is sorted, so a before b; c is elsewhere.
        assert_eq!(here, vec!["snakewood/mob/a#1", "snakewood/mob/b#1"]);
    }

    #[test]
    fn mob_location_tracks_current_room() {
        let mut realm = Realm::new(World::default());
        realm.insert_mob(mob_at("snakewood/mob/a#1", "snakewood/clearing"));
        let a = EntityId::new("snakewood/mob/a#1").unwrap();
        assert_eq!(
            realm.mob_location(&a).map(|r| r.as_str()),
            Some("snakewood/clearing")
        );
    }

    #[test]
    fn realm_has_default_no_exit_message() {
        let realm = Realm::new(World::default());
        assert_eq!(realm.no_exit_message, "You see no exit in that direction.");
        // Default delegates to new(World::default())
        assert_eq!(Realm::default().no_exit_message, realm.no_exit_message);
    }

    #[test]
    fn realm_starts_with_no_operators_and_default_rate_limit_message() {
        let realm = Realm::new(World::default());
        assert!(realm.operators.is_empty());
        assert_eq!(realm.rate_limit_message, "You can't do that yet.");
    }
}
