use std::collections::{BTreeMap, BTreeSet};

use snakewood_core::fabric::{Outcome, Rule, Trigger};
use snakewood_core::{
    Direction, EntityId, Flag, GitStore, Mob, Realm, Room, World, WorldStore,
};

fn id(s: &str) -> EntityId {
    EntityId::new(s).unwrap()
}

fn sample_realm() -> Realm {
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
    let mut realm = Realm::new(world);
    let mut flags = BTreeSet::new();
    flags.insert(Flag::Alive);
    flags.insert(Flag::Conscious);
    realm.insert_mob(Mob {
        id: id("snakewood/mob/goblin#1"),
        name: "a goblin".to_string(),
        location: id("snakewood/clearing"),
        flags,
        responders: Vec::new(),
    });
    realm.rules.push(Rule {
        on: Trigger::AnyMove,
        require: Vec::new(),
        effects: Vec::new(),
        outcome: Outcome::Allow,
        priority: 7,
    });
    realm
}

#[test]
fn realm_survives_a_git_clone() {
    let src = tempfile::tempdir().unwrap();
    let mut store = GitStore::init(src.path()).unwrap();
    let realm = sample_realm();
    store.save_realm(&realm).unwrap();
    store.commit("initial realm", 1_700_000_000).unwrap();

    // Clone the committed repo into a fresh directory and load from the clone.
    let dst = tempfile::tempdir().unwrap();
    git2::Repository::clone(src.path().to_str().unwrap(), dst.path()).unwrap();
    let reloaded = GitStore::init(dst.path()).unwrap().load_realm().unwrap();

    assert_eq!(reloaded.world, realm.world);
    assert_eq!(reloaded.mobs, realm.mobs);
    assert_eq!(reloaded.rules, realm.rules);
    assert_eq!(reloaded.no_exit_message, realm.no_exit_message);
}
