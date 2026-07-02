use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use snakewood_core::{Direction, EntityId, GitStore, Room};
use snakewood_daemon::telnet::{run_tick_loop, serve};
use snakewood_daemon::{Engine, SystemClock};
use tokio::net::TcpListener;

fn id(s: &str) -> EntityId {
    EntityId::new(s).expect("static id is valid")
}

/// Seed a minimal two-room world if the loaded realm has no rooms.
fn seed_if_empty(engine: &mut Engine) -> Result<(), Box<dyn std::error::Error>> {
    if !engine.realm().world.rooms.is_empty() {
        return Ok(());
    }
    let mut exits = BTreeMap::new();
    exits.insert(Direction::North, id("snakewood/old-well"));
    engine.realm_mut().world.insert_room(Room {
        id: id("snakewood/clearing"),
        name: "Snakewood Clearing".to_string(),
        description: "Gnarled snakewood trees ring a clearing of trampled grass.".to_string(),
        exits,
    });
    let mut back = BTreeMap::new();
    back.insert(Direction::South, id("snakewood/clearing"));
    engine.realm_mut().world.insert_room(Room {
        id: id("snakewood/old-well"),
        name: "The Old Well".to_string(),
        description: "A crumbling stone well sinks into darkness.".to_string(),
        exits: back,
    });
    engine
        .checkpoint("seed starter world")
        .map_err(|err| format!("{err:?}"))?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir =
        std::env::var("SNAKEWOOD_DATA").unwrap_or_else(|_| "./snakewood-data".to_string());
    let addr = std::env::var("SNAKEWOOD_ADDR").unwrap_or_else(|_| "127.0.0.1:4000".to_string());

    let store = GitStore::init(&data_dir).map_err(|err| format!("{err:?}"))?;
    let mut engine =
        Engine::boot(Box::new(store), Box::new(SystemClock)).map_err(|err| format!("{err:?}"))?;
    seed_if_empty(&mut engine)?;
    engine.set_snapshot_interval(3600);
    let engine = Rc::new(RefCell::new(engine));
    let start_room = id("snakewood/clearing");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let listener = TcpListener::bind(&addr).await?;
        eprintln!("snakewood listening on {addr}");
        tokio::task::spawn_local(run_tick_loop(engine.clone(), 1));
        serve(listener, engine, start_room).await;
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    Ok(())
}
