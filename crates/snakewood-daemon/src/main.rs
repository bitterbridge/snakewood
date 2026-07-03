use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use snakewood_core::{
    Direction, EntityId, GitStore, IntentClass, Operator, PresentationKind, Room, Scope,
};
use snakewood_daemon::api::serve_api;
use snakewood_daemon::telnet::{run_tick_loop, serve, RenderStyle};
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

/// Attach the M2 proof operators if none are configured: rate-limit movement
/// (one step per 4 ticks per actor) and coalesce redundant room redraws.
fn seed_operators_if_empty(engine: &mut Engine) -> Result<(), Box<dyn std::error::Error>> {
    if !engine.realm().operators.is_empty() {
        return Ok(());
    }
    engine.realm_mut().operators = vec![
        Operator::RateLimit {
            on: IntentClass::Move,
            per_ticks: 4,
            scope: Scope::PerActor,
            deny: Some("You catch your breath before moving again.".to_string()),
        },
        Operator::Coalesce {
            on: vec![
                PresentationKind::RoomName,
                PresentationKind::RoomDescription,
                PresentationKind::Exits,
                PresentationKind::Occupants,
            ],
            within_ticks: 1,
            scope: Scope::PerActor,
        },
    ];
    engine
        .checkpoint("seed M2 operators")
        .map_err(|err| format!("{err:?}"))?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir =
        std::env::var("SNAKEWOOD_DATA").unwrap_or_else(|_| "./snakewood-data".to_string());
    let addr = std::env::var("SNAKEWOOD_ADDR").unwrap_or_else(|_| "127.0.0.1:4000".to_string());
    let api_addr =
        std::env::var("SNAKEWOOD_API_ADDR").unwrap_or_else(|_| "127.0.0.1:4001".to_string());
    let tick_ms: u64 = std::env::var("SNAKEWOOD_TICK_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(250);
    let render_style = match std::env::var("SNAKEWOOD_ANSI").ok().as_deref() {
        Some("0") | Some("false") | Some("off") => RenderStyle::Plain,
        _ => RenderStyle::Ansi, // default on
    };

    let store = GitStore::init(&data_dir).map_err(|err| format!("{err:?}"))?;
    let mut engine =
        Engine::boot(Box::new(store), Box::new(SystemClock)).map_err(|err| format!("{err:?}"))?;
    seed_if_empty(&mut engine)?;
    seed_operators_if_empty(&mut engine)?;
    engine.set_snapshot_interval(3600);
    let engine = Rc::new(RefCell::new(engine));
    let start_room = id("snakewood/clearing");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let listener = TcpListener::bind(&addr).await?;
        let api_listener = TcpListener::bind(&api_addr).await?;
        eprintln!("snakewood telnet on {addr}, command API on {api_addr}");
        tokio::task::spawn_local(run_tick_loop(
            engine.clone(),
            Duration::from_millis(tick_ms),
        ));
        tokio::join!(
            serve(listener, engine.clone(), start_room.clone(), render_style),
            serve_api(api_listener, engine, start_room),
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    Ok(())
}
