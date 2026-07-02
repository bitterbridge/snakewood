use crate::fabric::{eval_predicate, Band, Effect, Intent, Outcome};
use crate::{EntityId, Realm};

/// A resolved handler contribution for one dispatch, after predicate filtering.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub band: Band,
    pub priority: i32,
    pub self_id: Option<EntityId>,
    pub outcome: Outcome,
    pub effects: Vec<Effect>,
}

fn require_passes(
    realm: &Realm,
    require: &[crate::fabric::Predicate],
    self_id: Option<&EntityId>,
    actor: &EntityId,
) -> bool {
    require
        .iter()
        .all(|p| eval_predicate(realm, p, self_id, actor))
}

/// Compute candidate handlers for `intent` from current state. Never cached.
pub fn gather(realm: &Realm, intent: &Intent) -> Vec<Candidate> {
    let actor = intent.actor();
    let mut candidates = Vec::new();

    let Some(room_id) = realm.mob_location(actor).cloned() else {
        return candidates; // actor not located; nothing to gather
    };

    // Structure band: sugar exits (Move only).
    if let Intent::Move { direction, .. } = intent {
        if let Some(room) = realm.world.room(&room_id) {
            if let Some(dest) = room.exits.get(direction) {
                candidates.push(Candidate {
                    band: Band::Structure,
                    priority: 0,
                    self_id: None,
                    outcome: Outcome::Traverse(dest.clone()),
                    effects: Vec::new(),
                });
            }
        }
    }

    // Participant band: co-present mobs (excluding the actor).
    for mob in realm.mobs_in_room(&room_id) {
        if &mob.id == actor {
            continue;
        }
        for responder in &mob.responders {
            if responder.on.matches(intent)
                && require_passes(realm, &responder.require, Some(&mob.id), actor)
            {
                candidates.push(Candidate {
                    band: Band::Participant,
                    priority: responder.priority,
                    self_id: Some(mob.id.clone()),
                    outcome: responder.outcome.clone(),
                    effects: responder.effects.clone(),
                });
            }
        }
    }

    // Global band: rules.
    for rule in &realm.rules {
        if rule.on.matches(intent) && require_passes(realm, &rule.require, None, actor) {
            candidates.push(Candidate {
                band: Band::Global,
                priority: rule.priority,
                self_id: None,
                outcome: rule.outcome.clone(),
                effects: rule.effects.clone(),
            });
        }
    }

    candidates
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::fabric::{Party, Predicate, Responder, Trigger};
    use crate::{Direction, Flag, Mob, Room, World};

    fn world_two_rooms() -> World {
        let mut exits = BTreeMap::new();
        exits.insert(
            Direction::North,
            EntityId::new("snakewood/old-well").unwrap(),
        );
        let mut world = World::default();
        world.insert_room(Room {
            id: EntityId::new("snakewood/clearing").unwrap(),
            name: "Clearing".to_string(),
            description: "A clearing.".to_string(),
            exits,
        });
        world.insert_room(Room {
            id: EntityId::new("snakewood/old-well").unwrap(),
            name: "Old Well".to_string(),
            description: "A well.".to_string(),
            exits: BTreeMap::new(),
        });
        world
    }

    fn actor_id() -> EntityId {
        EntityId::new("snakewood/pc/nathan").unwrap()
    }

    fn place_actor(realm: &mut Realm) {
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        realm.insert_mob(Mob {
            id: actor_id(),
            name: "Nathan".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        });
    }

    fn blocking_goblin(alive: bool) -> Mob {
        let mut flags = BTreeSet::new();
        if alive {
            flags.insert(Flag::Alive);
            flags.insert(Flag::Conscious);
        }
        Mob {
            id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
            name: "a goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: vec![Responder {
                on: Trigger::Move(Direction::North),
                require: vec![
                    Predicate::Alive(Party::SelfMob),
                    Predicate::Conscious(Party::SelfMob),
                ],
                effects: vec![Effect::Narrate(
                    Party::Actor,
                    "The goblin blocks your way north.".to_string(),
                )],
                outcome: Outcome::Block,
                priority: 0,
            }],
        }
    }

    #[test]
    fn gathers_sugar_exit_as_structure_traverse() {
        let mut realm = Realm::new(world_two_rooms());
        place_actor(&mut realm);
        let intent = Intent::Move {
            actor: actor_id(),
            direction: Direction::North,
        };
        let got = gather(&realm, &intent);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].band, Band::Structure);
        assert_eq!(
            got[0].outcome,
            Outcome::Traverse(EntityId::new("snakewood/old-well").unwrap())
        );
    }

    #[test]
    fn live_goblin_contributes_a_participant_block() {
        let mut realm = Realm::new(world_two_rooms());
        place_actor(&mut realm);
        realm.insert_mob(blocking_goblin(true));
        let intent = Intent::Move {
            actor: actor_id(),
            direction: Direction::North,
        };
        let got = gather(&realm, &intent);
        // sugar exit (Structure) + goblin block (Participant)
        assert_eq!(got.len(), 2);
        assert!(got
            .iter()
            .any(|c| c.band == Band::Participant && c.outcome == Outcome::Block));
    }

    #[test]
    fn dead_goblin_contributes_nothing_guarded_out() {
        let mut realm = Realm::new(world_two_rooms());
        place_actor(&mut realm);
        realm.insert_mob(blocking_goblin(false)); // no Alive/Conscious flags
        let intent = Intent::Move {
            actor: actor_id(),
            direction: Direction::North,
        };
        let got = gather(&realm, &intent);
        // only the sugar exit; the goblin's require predicates fail
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].band, Band::Structure);
    }
}
