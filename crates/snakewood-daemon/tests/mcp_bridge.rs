use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{json, Value};
use snakewood_core::{Direction, EntityId, Realm, Room, World};
use snakewood_daemon::api::serve_api;
use snakewood_daemon::mcp::{dispatch_rpc, JsonRpcRequest, TcpDaemonClient};
use snakewood_daemon::telnet::run_tick_loop;
use snakewood_daemon::{Engine, ManualClock};

fn id(s: &str) -> EntityId {
    EntityId::new(s).unwrap()
}

fn two_room_world() -> World {
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

fn rpc(method: &str, id_num: i64, params: Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(Value::from(id_num)),
        method: method.to_string(),
        params: Some(params),
    }
}

#[test]
fn mcp_bridge_drives_the_daemon() {
    // Start the daemon API on a background thread with its own current-thread runtime.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            let engine = Rc::new(RefCell::new(Engine::new(
                Realm::new(two_room_world()),
                Box::new(ManualClock::new(0)),
            )));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            tokio::task::spawn_local(run_tick_loop(engine.clone(), Duration::from_millis(20)));
            serve_api(listener, engine, id("snakewood/clearing")).await;
        });
    });
    let addr = rx.recv().unwrap().to_string();

    // Bridge client connects as the persistent builder.
    let mut client = TcpDaemonClient::connect(&addr, "player/mcp-builder").unwrap();

    // initialize
    let init = dispatch_rpc(
        &rpc("initialize", 1, Value::Null),
        client.session,
        &mut client,
    )
    .unwrap();
    assert_eq!(init.result.unwrap()["serverInfo"]["name"], "snakewood");

    // tools/list
    let list = dispatch_rpc(
        &rpc("tools/list", 2, Value::Null),
        client.session,
        &mut client,
    )
    .unwrap();
    assert_eq!(list.result.unwrap()["tools"].as_array().unwrap().len(), 3);

    // tools/call snakewood_dig east
    let dig = dispatch_rpc(
        &rpc(
            "tools/call",
            3,
            json!({
                "name": "snakewood_dig",
                "arguments": {"direction":"east","id":"snakewood/hollow","name":"A Hollow","description":"Mossy."}
            }),
        ),
        client.session,
        &mut client,
    )
    .unwrap();
    let dig_text = dig.result.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        dig_text.contains("east"),
        "dig view should list the new east exit: {dig_text}"
    );

    // tools/call snakewood_move east -> into the dug room
    let mv = dispatch_rpc(
        &rpc(
            "tools/call",
            4,
            json!({"name": "snakewood_move", "arguments": {"direction":"east"}}),
        ),
        client.session,
        &mut client,
    )
    .unwrap();
    let mv_text = mv.result.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        mv_text.contains("A Hollow"),
        "move should arrive at the dug room: {mv_text}"
    );
}
