use std::cell::RefCell;
use std::rc::Rc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use snakewood_core::EntityId;

use crate::api::{handle_api_request, ApiRequest, ApiResponse};
use crate::telnet::despawn_player;
use crate::{Engine, SessionId};

/// Accept structured-API connections forever; handle each on a local task.
pub async fn serve_api(listener: TcpListener, engine: Rc<RefCell<Engine>>, start_room: EntityId) {
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(_) => continue,
        };
        let engine = engine.clone();
        let start_room = start_room.clone();
        tokio::task::spawn_local(async move {
            let _ = handle_api_connection(stream, engine, start_room).await;
        });
    }
}

/// Drive one structured-API connection.
///
/// Mirrors telnet's always-cleanup pattern: every player spawned on this
/// connection is despawned when the connection ends, whether that's a clean
/// EOF, an explicit `Disconnect`, or the TCP stream dropping mid-read.
async fn handle_api_connection(
    stream: TcpStream,
    engine: Rc<RefCell<Engine>>,
    start_room: EntityId,
) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    // Sessions this connection has spawned but not yet explicitly disconnected.
    let mut created: Vec<SessionId> = Vec::new();

    let result: std::io::Result<()> = async {
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            // Parse, then dispatch in a tight borrow block (no borrow across await).
            let response = match serde_json::from_str::<ApiRequest>(&line) {
                Ok(req) => {
                    // A Disconnect despawns inside handle_api_request itself, so
                    // stop tracking that session here to avoid a double despawn.
                    let disconnecting = match &req {
                        ApiRequest::Disconnect { session } => Some(SessionId(*session)),
                        _ => None,
                    };
                    let mut e = engine.borrow_mut();
                    let resp = handle_api_request(&mut e, req, &start_room);
                    if let ApiResponse::Connected { session, .. } = &resp {
                        created.push(SessionId(*session));
                    }
                    if let Some(sid) = disconnecting {
                        created.retain(|s| *s != sid);
                    }
                    resp
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
    .await;

    // ALWAYS despawn any players this connection created, regardless of
    // clean exit, explicit disconnect, or I/O error.
    {
        let mut e = engine.borrow_mut();
        for sid in created {
            if let Some(actor) = e.session_actor(sid).cloned() {
                despawn_player(&mut e, sid, &actor);
            }
        }
    }
    result
}
