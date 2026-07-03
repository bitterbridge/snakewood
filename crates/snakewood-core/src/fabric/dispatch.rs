use crate::fabric::{
    gather, resolve, salient, Candidate, Decision, Effect, Event, Intent, Outcome, Party,
};
use crate::{EntityId, PresentationNode, Realm};

/// The result of dispatching one intent: committed events + directed messages.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dispatch {
    pub events: Vec<Event>,
    pub messages: Vec<(EntityId, PresentationNode)>,
}

/// Resolve a Party to a concrete recipient id for an effect.
fn recipient(party: Party, self_id: Option<&EntityId>, actor: &EntityId) -> Option<EntityId> {
    match party {
        Party::Actor => Some(actor.clone()),
        Party::SelfMob => self_id.cloned(),
    }
}

/// Turn a salient candidate's effects into directed messages.
fn apply_effects(cand: &Candidate, actor: &EntityId, out: &mut Vec<(EntityId, PresentationNode)>) {
    for effect in &cand.effects {
        match effect {
            Effect::Narrate(party, text) => {
                if let Some(to) = recipient(*party, cand.self_id.as_ref(), actor) {
                    out.push((to, PresentationNode::Line(crate::plain_text(text.clone()))));
                }
            }
        }
    }
}

/// Build the semantic view of a room for `viewer` (excludes the viewer from occupants).
fn room_presentation(
    realm: &Realm,
    room_id: &EntityId,
    viewer: &EntityId,
) -> Vec<PresentationNode> {
    let mut nodes = Vec::new();
    if let Some(room) = realm.world.room(room_id) {
        nodes.push(PresentationNode::RoomName(room.name.clone()));
        nodes.push(PresentationNode::RoomDescription(crate::plain_text(
            room.description.clone(),
        )));
        nodes.push(PresentationNode::Exits(
            room.exits.keys().cloned().collect(),
        ));
        let occupants: Vec<crate::Span> = {
            let mut names: Vec<String> = realm
                .mobs_in_room(room_id)
                .iter()
                .filter(|m| &m.id != viewer)
                .map(|m| m.name.clone())
                .collect();
            names.sort();
            names.into_iter().map(crate::Span::actor).collect()
        };
        nodes.push(PresentationNode::Occupants(occupants));
    }
    nodes
}

/// Dispatch one intent through Guard -> Commit -> Notify.
pub fn dispatch(realm: &mut Realm, intent: Intent) -> Dispatch {
    let mut out = Dispatch::default();
    let actor = intent.actor().clone();

    match &intent {
        Intent::Look { .. } => {
            // Look is NOT guarded in Stage 2: it never calls gather()/resolve(),
            // so `Trigger::Look` responders/rules are inert for now. Wiring Look
            // through Guard (e.g. predicate-gated observation like blindness) is a
            // Stage 3 decision — do not assume Look handlers fire yet.
            if let Some(room_id) = realm.mob_location(&actor).cloned() {
                out.events.push(Event::Looked {
                    actor: actor.clone(),
                    room: room_id.clone(),
                });
                for node in room_presentation(realm, &room_id, &actor) {
                    out.messages.push((actor.clone(), node));
                }
            }
        }
        Intent::Move { .. } => {
            let from = match realm.mob_location(&actor).cloned() {
                Some(r) => r,
                None => return out, // unlocated actor: nothing happens
            };
            // GUARD
            let candidates = gather(realm, &intent);
            match resolve(&candidates) {
                Decision::Denied => {
                    // salient among the blockers narrates/reacts
                    let blockers: Vec<Candidate> = candidates
                        .iter()
                        .filter(|c| c.outcome == Outcome::Block)
                        .cloned()
                        .collect();
                    if let Some(s) = salient(&blockers) {
                        apply_effects(s, &actor, &mut out.messages);
                    }
                }
                Decision::Allowed { destination } => {
                    // salient traverser may carry effects (usually none for a plain exit)
                    let traversers: Vec<Candidate> = candidates
                        .iter()
                        .filter(|c| matches!(c.outcome, Outcome::Traverse(_)))
                        .cloned()
                        .collect();
                    if let Some(s) = salient(&traversers) {
                        apply_effects(s, &actor, &mut out.messages);
                    }
                    // COMMIT
                    if let Some(mob) = realm.mob_mut(&actor) {
                        mob.location = destination.clone();
                    }
                    out.events.push(Event::Moved {
                        actor: actor.clone(),
                        from,
                        to: destination.clone(),
                    });
                    // arrival view
                    for node in room_presentation(realm, &destination, &actor) {
                        out.messages.push((actor.clone(), node));
                    }
                }
                Decision::Unresolved => {
                    out.messages.push((
                        actor.clone(),
                        PresentationNode::Denied(crate::plain_text(realm.no_exit_message.clone())),
                    ));
                }
            }
            // NOTIFY: Stage 2 publishes events (already in `out`). Observer-driven
            // reactions (mobs emitting follow-up intents) are deferred to Stage 3.
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
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
            name: "Snakewood Clearing".to_string(),
            description: "A clearing.".to_string(),
            exits,
        });
        world.insert_room(Room {
            id: EntityId::new("snakewood/old-well").unwrap(),
            name: "The Old Well".to_string(),
            description: "A crumbling well.".to_string(),
            exits: BTreeMap::new(),
        });
        world
    }

    fn actor_id() -> EntityId {
        EntityId::new("snakewood/pc/nathan").unwrap()
    }

    fn realm_with_actor() -> Realm {
        let mut realm = Realm::new(world_two_rooms());
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        realm.insert_mob(Mob {
            id: actor_id(),
            name: "Nathan".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        });
        realm
    }

    #[test]
    fn move_through_open_exit_relocates_and_emits_moved() {
        let mut realm = realm_with_actor();
        let out = dispatch(
            &mut realm,
            Intent::Move {
                actor: actor_id(),
                direction: Direction::North,
            },
        );
        assert_eq!(
            realm.mob_location(&actor_id()).map(|r| r.as_str()),
            Some("snakewood/old-well")
        );
        assert!(out.events.contains(&Event::Moved {
            actor: actor_id(),
            from: EntityId::new("snakewood/clearing").unwrap(),
            to: EntityId::new("snakewood/old-well").unwrap(),
        }));
        // arrival view names the new room
        assert!(out
            .messages
            .iter()
            .any(|(_, n)| *n == PresentationNode::RoomName("The Old Well".to_string())));
    }

    #[test]
    fn move_with_no_exit_is_denied_with_fallback_message() {
        let mut realm = realm_with_actor();
        let out = dispatch(
            &mut realm,
            Intent::Move {
                actor: actor_id(),
                direction: Direction::South,
            },
        );
        assert_eq!(
            realm.mob_location(&actor_id()).map(|r| r.as_str()),
            Some("snakewood/clearing")
        );
        assert!(out.messages.iter().any(|(_, n)| *n
            == PresentationNode::Denied(crate::plain_text("You see no exit in that direction."))));
        assert!(out.events.is_empty());
    }

    #[test]
    fn no_exit_message_is_data_driven() {
        let mut realm = realm_with_actor();
        realm.no_exit_message = "There's nothing that way, friend.".to_string();
        let out = dispatch(
            &mut realm,
            Intent::Move {
                actor: actor_id(),
                direction: Direction::South,
            },
        );
        assert!(out.messages.iter().any(|(_, n)| *n
            == PresentationNode::Denied(crate::plain_text("There's nothing that way, friend."))));
    }

    #[test]
    fn look_produces_room_view_and_looked_event() {
        let mut realm = realm_with_actor();
        let out = dispatch(&mut realm, Intent::Look { actor: actor_id() });
        assert!(out.events.contains(&Event::Looked {
            actor: actor_id(),
            room: EntityId::new("snakewood/clearing").unwrap(),
        }));
        assert!(out
            .messages
            .iter()
            .any(|(_, n)| *n == PresentationNode::RoomName("Snakewood Clearing".to_string())));
    }
}
