use std::collections::{BTreeMap, BTreeSet};

use snakewood_core::{Direction, EntityId, Flag, GitStore, Intent, Mob, Realm, Room, World};
use snakewood_daemon::{Engine, ManualClock, SessionId};

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

fn seeded_realm() -> Realm {
    let mut realm = Realm::new(world());
    let mut flags = BTreeSet::new();
    flags.insert(Flag::Alive);
    realm.insert_mob(Mob {
        id: id("snakewood/pc/nathan"),
        name: "Nathan".to_string(),
        location: id("snakewood/clearing"),
        flags,
        responders: Vec::new(),
    });
    realm
}

#[test]
fn live_position_survives_a_daemon_restart() {
    let dir = tempfile::tempdir().unwrap();

    // First run: connect, walk north, checkpoint, then the engine goes away.
    {
        let store = GitStore::init(dir.path()).unwrap();
        let mut engine = Engine::new(seeded_realm(), Box::new(ManualClock::new(1000)));
        engine.attach_store(Box::new(store));
        let sid = engine.connect(id("snakewood/pc/nathan"));
        engine.enqueue(
            sid,
            Intent::Move {
                actor: id("snakewood/pc/nathan"),
                direction: Direction::North,
            },
        );
        engine.tick();
        assert_eq!(
            engine
                .realm()
                .mob_location(&id("snakewood/pc/nathan"))
                .map(|r| r.as_str()),
            Some("snakewood/old-well")
        );
        engine.checkpoint("nathan walked north").unwrap();
    }

    // Second run: boot from the same repo. Position persisted; no sessions.
    let store = GitStore::init(dir.path()).unwrap();
    let engine = Engine::boot(Box::new(store), Box::new(ManualClock::new(5000))).unwrap();
    assert_eq!(
        engine
            .realm()
            .mob_location(&id("snakewood/pc/nathan"))
            .map(|r| r.as_str()),
        Some("snakewood/old-well")
    );
    assert_eq!(engine.session_actor(SessionId(0)), None);
}
