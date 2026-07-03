use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use snakewood_core::EntityId;

use crate::api::{build_drain_response, handle_api_request, ApiOutcome, ApiRequest, ApiResponse};
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

/// How a request's resulting session (if any) should be cleaned up when the
/// connection ends.
#[derive(Clone, Copy)]
enum RequestKind {
    /// `Connect`: despawn (mob + session) on cleanup.
    Ephemeral,
    /// `ConnectAs`: disconnect the session only; the named mob persists.
    Persistent,
    /// `Disconnect { session }`: already handled inline; stop tracking it.
    Disconnected(SessionId),
    /// Anything else: doesn't affect tracking.
    Other,
}

/// Poll until the engine has completed at least one drain past `before`, or a
/// safety timeout elapses (a stuck tick loop must not hang the connection).
async fn wait_for_drain(engine: &Rc<RefCell<Engine>>, before: u64) {
    for _ in 0..300 {
        if engine.borrow().drain_count() > before {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Drive one structured-API connection.
///
/// Mirrors telnet's always-cleanup pattern: every player spawned on this
/// connection is cleaned up when the connection ends, whether that's a clean
/// EOF, an explicit `Disconnect`, or the TCP stream dropping mid-read.
/// `Connect`-spawned (ephemeral) players are despawned entirely; `ConnectAs`
/// (persistent, named) actors only have their session disconnected — their
/// mob survives so a later `ConnectAs` can reattach to it.
async fn handle_api_connection(
    stream: TcpStream,
    engine: Rc<RefCell<Engine>>,
    start_room: EntityId,
) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    // Sessions this connection has spawned but not yet explicitly disconnected.
    let mut ephemeral: Vec<SessionId> = Vec::new();
    let mut persistent: Vec<SessionId> = Vec::new();

    let result: std::io::Result<()> = async {
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            // Parse once, then dispatch in a tight borrow block (no borrow across await).
            let parsed = serde_json::from_str::<ApiRequest>(&line);
            let response = match parsed {
                Ok(req) => {
                    // Classify the request so cleanup knows how to treat any
                    // session it creates. A Disconnect despawns inside
                    // handle_api_request itself, so stop tracking that
                    // session here to avoid a double despawn.
                    let kind = match &req {
                        ApiRequest::Connect => RequestKind::Ephemeral,
                        ApiRequest::ConnectAs { .. } => RequestKind::Persistent,
                        ApiRequest::Disconnect { session } => {
                            RequestKind::Disconnected(SessionId(*session))
                        }
                        _ => RequestKind::Other,
                    };
                    let outcome = {
                        let mut e = engine.borrow_mut();
                        handle_api_request(&mut e, req, &start_room)
                    };
                    // Track any session this request created, for cleanup.
                    if let ApiOutcome::AwaitDrain { session, .. } = &outcome {
                        match kind {
                            RequestKind::Ephemeral => ephemeral.push(*session),
                            RequestKind::Persistent => persistent.push(*session),
                            _ => {}
                        }
                    }
                    if let RequestKind::Disconnected(sid) = kind {
                        ephemeral.retain(|s| *s != sid);
                        persistent.retain(|s| *s != sid);
                    }
                    match outcome {
                        ApiOutcome::Ready(resp) => resp,
                        ApiOutcome::AwaitDrain {
                            session,
                            before,
                            shape,
                        } => {
                            wait_for_drain(&engine, before).await;
                            let view = {
                                let mut e = engine.borrow_mut();
                                e.poll(session)
                            };
                            build_drain_response(shape, session, view)
                        }
                    }
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

    // ALWAYS clean up any sessions this connection created, regardless of
    // clean exit, explicit disconnect, or I/O error. Ephemeral players are
    // despawned entirely; persistent (named) actors only lose their session.
    {
        let mut e = engine.borrow_mut();
        for sid in ephemeral {
            if let Some(actor) = e.session_actor(sid).cloned() {
                despawn_player(&mut e, sid, &actor);
            }
        }
        for sid in persistent {
            e.disconnect(sid);
        }
    }
    result
}
