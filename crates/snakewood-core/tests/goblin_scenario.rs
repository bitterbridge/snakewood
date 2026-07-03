use std::collections::{BTreeMap, BTreeSet};

use snakewood_core::fabric::{Effect, Outcome, Party, Predicate, Responder, Trigger};
use snakewood_core::{
    dispatch, Direction, EntityId, Event, Flag, Intent, Mob, PresentationNode, Realm, Room, World,
};

fn id(s: &str) -> EntityId {
    EntityId::new(s).unwrap()
}

fn world() -> World {
    let mut exits = BTreeMap::new();
    exits.insert(Direction::North, id("snakewood/old-well"));
    let mut world = World::default();
    world.insert_room(Room {
        id: id("snakewood/clearing"),
        name: "Snakewood Clearing".to_string(),
        description: "Gnarled snakewood trees ring a clearing.".to_string(),
        exits,
    });
    world.insert_room(Room {
        id: id("snakewood/old-well"),
        name: "The Old Well".to_string(),
        description: "A crumbling stone well.".to_string(),
        exits: BTreeMap::new(),
    });
    world
}

fn actor() -> Mob {
    let mut flags = BTreeSet::new();
    flags.insert(Flag::Alive);
    Mob {
        id: id("snakewood/pc/nathan"),
        name: "Nathan".to_string(),
        location: id("snakewood/clearing"),
        flags,
        responders: Vec::new(),
    }
}

fn goblin() -> Mob {
    let mut flags = BTreeSet::new();
    flags.insert(Flag::Alive);
    flags.insert(Flag::Conscious);
    Mob {
        id: id("snakewood/mob/goblin#1"),
        name: "a snakewood goblin".to_string(),
        location: id("snakewood/clearing"),
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

fn realm() -> Realm {
    let mut realm = Realm::new(world());
    realm.insert_mob(actor());
    realm.insert_mob(goblin());
    realm
}

#[test]
fn conscious_goblin_blocks_north_with_salient_message() {
    let mut realm = realm();
    let out = dispatch(
        &mut realm,
        Intent::Move {
            actor: id("snakewood/pc/nathan"),
            direction: Direction::North,
        },
    );

    // Actor did NOT move.
    assert_eq!(
        realm
            .mob_location(&id("snakewood/pc/nathan"))
            .map(|r| r.as_str()),
        Some("snakewood/clearing")
    );
    // No Moved event.
    assert!(!out.events.iter().any(|e| matches!(e, Event::Moved { .. })));
    // The salient (Participant) narration is the goblin's, delivered to the actor.
    assert!(out.messages.contains(&(
        id("snakewood/pc/nathan"),
        PresentationNode::Line(snakewood_core::plain_text(
            "The goblin blocks your way north."
        ))
    )));
}

#[test]
fn incapacitated_goblin_stops_blocking_no_wiring_change() {
    let mut realm = realm();
    // Knock the goblin unconscious — pure state change, no subscription edits.
    realm
        .mob_mut(&id("snakewood/mob/goblin#1"))
        .unwrap()
        .flags
        .remove(&Flag::Conscious);

    let out = dispatch(
        &mut realm,
        Intent::Move {
            actor: id("snakewood/pc/nathan"),
            direction: Direction::North,
        },
    );

    // Now the actor moves through.
    assert_eq!(
        realm
            .mob_location(&id("snakewood/pc/nathan"))
            .map(|r| r.as_str()),
        Some("snakewood/old-well")
    );
    assert!(out.events.contains(&Event::Moved {
        actor: id("snakewood/pc/nathan"),
        from: id("snakewood/clearing"),
        to: id("snakewood/old-well"),
    }));
    // The goblin's block message is absent.
    assert!(!out.messages.iter().any(|(_, n)| *n
        == PresentationNode::Line(snakewood_core::plain_text(
            "The goblin blocks your way north."
        ))));
}
