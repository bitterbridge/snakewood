use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use snakewood_core::{Direction, EntityId, Realm, Room, World};
use snakewood_daemon::telnet::{run_tick_loop, serve};
use snakewood_daemon::{Engine, ManualClock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn id(s: &str) -> EntityId {
    EntityId::new(s).unwrap()
}

fn two_room_engine() -> Engine {
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
    Engine::new(Realm::new(world), Box::new(ManualClock::new(0)))
}

/// Read whatever arrives within `ms` into a string (best-effort).
async fn read_for(stream: &mut TcpStream, ms: u64) -> String {
    let mut buf = vec![0u8; 4096];
    let mut acc = String::new();
    loop {
        match tokio::time::timeout(Duration::from_millis(ms), stream.read(&mut buf)).await {
            Ok(Ok(0)) => break, // EOF
            Ok(Ok(n)) => acc.push_str(&String::from_utf8_lossy(&buf[..n])),
            Ok(Err(_)) => break, // read error
            Err(_) => break,     // timeout: assume no more for now
        }
    }
    acc
}

#[test]
fn connect_look_and_walk() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let engine = Rc::new(RefCell::new(two_room_engine()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::task::spawn_local(serve(listener, engine.clone(), id("snakewood/clearing")));
        tokio::task::spawn_local(run_tick_loop(engine, Duration::from_millis(20)));

        let mut client = TcpStream::connect(addr).await.unwrap();

        // Greeting shows the start room.
        let greeting = read_for(&mut client, 500).await;
        assert!(
            greeting.contains("Snakewood Clearing"),
            "greeting was: {greeting:?}"
        );

        // Walk north -> arrive at the Old Well.
        client.write_all(b"n\r\n").await.unwrap();
        let after_move = read_for(&mut client, 500).await;
        assert!(
            after_move.contains("The Old Well"),
            "after_move was: {after_move:?}"
        );

        // Unknown command -> What?
        client.write_all(b"fluffernuts\r\n").await.unwrap();
        let after_unknown = read_for(&mut client, 500).await;
        assert!(
            after_unknown.contains("What?"),
            "after_unknown was: {after_unknown:?}"
        );
    });
}
