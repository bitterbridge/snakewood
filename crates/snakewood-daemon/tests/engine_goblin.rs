use std::collections::{BTreeMap, BTreeSet};

use snakewood_core::fabric::{Effect, Outcome, Party, Predicate, Responder, Trigger};
use snakewood_core::{
    Direction, EntityId, Flag, Intent, Mob, PresentationNode, Realm, Room, World,
};
use snakewood_daemon::{Engine, ManualClock};

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
        description: "A clearing.".to_string(),
        exits,
    });
    world.insert_room(Room {
        id: id("snakewood/old-well"),
        name: "The Old Well".to_string(),
        description: "A well.".to_string(),
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

fn engine_with_scene() -> Engine {
    let mut realm = Realm::new(world());
    realm.insert_mob(actor());
    realm.insert_mob(goblin());
    Engine::new(realm, Box::new(ManualClock::new(0)))
}

#[test]
fn engine_delivers_block_then_passage_after_incapacitation() {
    let mut e = engine_with_scene();
    let sid = e.connect(id("snakewood/pc/nathan"));

    // Conscious goblin blocks: the session receives the block line, no relocation.
    e.submit(
        sid,
        Intent::Move {
            actor: id("snakewood/pc/nathan"),
            direction: Direction::North,
        },
    );
    assert_eq!(
        e.realm()
            .mob_location(&id("snakewood/pc/nathan"))
            .map(|r| r.as_str()),
        Some("snakewood/clearing")
    );
    let view = e.poll(sid);
    assert!(view.contains(&PresentationNode::Line(
        "The goblin blocks your way north.".to_string()
    )));

    // Knock the goblin unconscious — pure state change on the realm, no wiring edits.
    e.realm_mut()
        .mob_mut(&id("snakewood/mob/goblin#1"))
        .unwrap()
        .flags
        .remove(&Flag::Conscious);

    // Same intent now passes; arrival view delivered.
    e.submit(
        sid,
        Intent::Move {
            actor: id("snakewood/pc/nathan"),
            direction: Direction::North,
        },
    );
    assert_eq!(
        e.realm()
            .mob_location(&id("snakewood/pc/nathan"))
            .map(|r| r.as_str()),
        Some("snakewood/old-well")
    );
    let view = e.poll(sid);
    assert!(view.contains(&PresentationNode::RoomName("The Old Well".to_string())));
    assert!(!view.contains(&PresentationNode::Line(
        "The goblin blocks your way north.".to_string()
    )));
}
