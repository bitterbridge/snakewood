use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use snakewood_core::{Direction, EntityId, PresentationNode, Realm, Room, World};
use snakewood_daemon::api::{serve_api, ApiRequest, ApiResponse};
use snakewood_daemon::telnet::run_tick_loop;
use snakewood_daemon::{Engine, ManualClock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

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

/// Send one request line and read one response line (with a timeout).
async fn exchange(
    write_half: &mut OwnedWriteHalf,
    lines: &mut tokio::io::Lines<BufReader<OwnedReadHalf>>,
    req: &ApiRequest,
) -> ApiResponse {
    let mut line = serde_json::to_string(req).unwrap();
    line.push('\n');
    write_half.write_all(line.as_bytes()).await.unwrap();
    let resp_line = tokio::time::timeout(Duration::from_millis(500), lines.next_line())
        .await
        .expect("timed out waiting for response")
        .unwrap()
        .expect("connection closed");
    serde_json::from_str(&resp_line).unwrap()
}

#[test]
fn drive_world_over_json_api() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let engine = Rc::new(RefCell::new(two_room_engine()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::task::spawn_local(serve_api(
            listener,
            engine.clone(),
            id("snakewood/clearing"),
        ));
        tokio::task::spawn_local(run_tick_loop(engine, Duration::from_millis(20)));

        // Single connection, split into reader + writer halves that share the socket.
        let stream = TcpStream::connect(addr).await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut lines = BufReader::new(read_half).lines();

        // Connect.
        let connected = exchange(&mut write_half, &mut lines, &ApiRequest::Connect).await;
        let session = match connected {
            ApiResponse::Connected {
                session, ref view, ..
            } => {
                assert!(view.contains(&PresentationNode::RoomName(
                    "Snakewood Clearing".to_string()
                )));
                session
            }
            other => panic!("expected Connected, got {other:?}"),
        };

        // Dig east into a new hollow; the returned view lists an east exit.
        let dug = exchange(
            &mut write_half,
            &mut lines,
            &ApiRequest::Dig {
                session,
                direction: Direction::East,
                id: "snakewood/hollow".to_string(),
                name: "A Hollow".to_string(),
                description: "A mossy hollow.".to_string(),
            },
        )
        .await;
        match dug {
            ApiResponse::Ok { messages } => assert!(messages.iter().any(
                |n| matches!(n, PresentationNode::Exits(dirs) if dirs.contains(&Direction::East))
            )),
            other => panic!("expected Ok from dig, got {other:?}"),
        }

        // Move east into the room we just dug.
        let moved = exchange(
            &mut write_half,
            &mut lines,
            &ApiRequest::Move {
                session,
                direction: Direction::East,
            },
        )
        .await;
        match moved {
            ApiResponse::Ok { messages } => {
                assert!(messages.contains(&PresentationNode::RoomName("A Hollow".to_string())))
            }
            other => panic!("expected Ok from move, got {other:?}"),
        }
    });
}
