use std::cell::RefCell;
use std::rc::Rc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use snakewood_core::EntityId;

use crate::api::{handle_api_request, ApiRequest, ApiResponse};
use crate::Engine;

/// Accept structured-API connections forever; handle each on a local task.
pub async fn serve_api(listener: TcpListener, engine: Rc<RefCell<Engine>>, start_room: EntityId) {
    // Player sequence shared across all API connections.
    let next_player = Rc::new(RefCell::new(0u64));
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(_) => continue,
        };
        let engine = engine.clone();
        let start_room = start_room.clone();
        let next_player = next_player.clone();
        tokio::task::spawn_local(async move {
            let _ = handle_api_connection(stream, engine, start_room, next_player).await;
        });
    }
}

async fn handle_api_connection(
    stream: TcpStream,
    engine: Rc<RefCell<Engine>>,
    start_room: EntityId,
    next_player: Rc<RefCell<u64>>,
) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        // Parse, then dispatch in a tight borrow block (no borrow across await).
        let response = match serde_json::from_str::<ApiRequest>(&line) {
            Ok(req) => {
                let mut e = engine.borrow_mut();
                let mut seq = next_player.borrow_mut();
                handle_api_request(&mut e, req, &start_room, &mut seq)
            }
            Err(err) => ApiResponse::Error {
                message: format!("bad request: {err}"),
            },
        };
        let mut out = serde_json::to_string(&response).unwrap_or_else(|_| {
            "{\"status\":\"error\",\"message\":\"serialize failed\"}".to_string()
        });
        out.push('\n');
        write_half.write_all(out.as_bytes()).await?;
    }
    Ok(())
}
