use std::collections::BTreeMap;

use proptest::prelude::*;
use tempfile::tempdir;

use snakewood_core::{Direction, EntityId, GitStore, Room, World, WorldStore};

// Strategy for a valid id name segment: 1-8 chars from [a-z0-9-].
fn name_seg() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-z][a-z0-9-]{0,7}").unwrap()
}

fn arb_room(zone: &'static str) -> impl Strategy<Value = Room> {
    (
        name_seg(),
        ".*",
        prop::collection::btree_map(
            prop_oneof![
                Just(Direction::North),
                Just(Direction::South),
                Just(Direction::East),
                Just(Direction::West),
                Just(Direction::Up),
                Just(Direction::Down),
            ],
            name_seg(),
            0..6,
        ),
    )
        .prop_map(move |(name, desc, exit_names)| {
            let mut exits = BTreeMap::new();
            for (dir, target) in exit_names {
                exits.insert(dir, EntityId::new(format!("{zone}/{target}")).unwrap());
            }
            Room {
                id: EntityId::new(format!("{zone}/{name}")).unwrap(),
                name,
                description: desc,
                exits,
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_world_round_trips_through_git(rooms in prop::collection::vec(arb_room("snakewood"), 1..8)) {
        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();

        let mut expected = World::default();
        for room in &rooms {
            store.save_room(room).unwrap();
            expected.insert_room(room.clone());
        }
        store.commit("proptest world", 1_700_000_000).unwrap();

        let reloaded = GitStore::init(dir.path()).unwrap().load_all().unwrap();
        prop_assert_eq!(reloaded, expected);
    }
}

#[test]
fn known_room_matches_golden() {
    use snakewood_core::room_to_ron;

    let mut exits = BTreeMap::new();
    exits.insert(
        Direction::North,
        EntityId::new("snakewood/old-well").unwrap(),
    );
    let room = Room {
        id: EntityId::new("snakewood/clearing").unwrap(),
        name: "Snakewood Clearing".to_string(),
        description: "Gnarled snakewood trees ring a clearing.".to_string(),
        exits,
    };

    let actual = room_to_ron(&room);
    if std::env::var("TROUBLESHOOT").is_ok() {
        eprintln!("--- actual room_to_ron output ---\n{actual}\n--- end ---");
    }
    let golden = include_str!("golden/clearing.ron");
    assert_eq!(
        actual.trim(),
        golden.trim(),
        "serialized room drifted from golden file"
    );
}
