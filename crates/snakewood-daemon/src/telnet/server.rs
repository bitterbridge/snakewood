use std::cell::RefCell;
use std::rc::Rc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use snakewood_core::{EntityId, Intent};

use crate::telnet::{despawn_player, is_quit, parse, render, spawn_player};
use crate::Engine;

/// Accept connections forever, handling each on a local task.
pub async fn serve(listener: TcpListener, engine: Rc<RefCell<Engine>>, start_room: EntityId) {
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(_) => continue,
        };
        let engine = engine.clone();
        let start_room = start_room.clone();
        tokio::task::spawn_local(async move {
            let _ = handle_connection(stream, engine, start_room).await;
        });
    }
}

/// Drive one player's connection: a cancel-safe reader task feeds parsed lines
/// over a channel; the main loop selects between incoming lines and a flush
/// interval that delivers drained presentation.
async fn handle_connection(
    stream: TcpStream,
    engine: Rc<RefCell<Engine>>,
    start_room: EntityId,
) -> std::io::Result<()> {
    use std::time::Duration;

    let (read_half, mut write_half) = stream.into_split();

    // Spawn the player up front so cleanup can always run.
    let (sid, actor) = {
        let mut e = engine.borrow_mut();
        spawn_player(&mut e, &start_room)
    };

    // Greet with a Look (delivered after the next drain via the flush arm).
    {
        let mut e = engine.borrow_mut();
        e.enqueue(
            sid,
            Intent::Look {
                actor: actor.clone(),
            },
        );
    }

    // Cancel-safe line reader: `next_line` is NOT cancel-safe, so it lives in a
    // dedicated task that forwards complete lines over a channel.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::task::spawn_local(async move {
        let mut lines = BufReader::new(read_half).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut flush = tokio::time::interval(Duration::from_millis(50));

    let result: std::io::Result<()> = async {
        loop {
            tokio::select! {
                maybe_line = rx.recv() => {
                    let line = match maybe_line {
                        Some(l) => l,
                        None => break, // reader task ended (EOF / disconnect)
                    };
                    if is_quit(&line) {
                        break;
                    }
                    // Unknown verbs never form an intent, so answer immediately.
                    let immediate = {
                        let mut e = engine.borrow_mut();
                        match parse(&line, &actor) {
                            Some(intent) => {
                                e.enqueue(sid, intent);
                                None
                            }
                            None if line.trim().is_empty() => Some(String::new()),
                            None => Some("What?\r\n".to_string()),
                        }
                    };
                    if let Some(reply) = immediate {
                        if !reply.is_empty() {
                            write_half.write_all(reply.as_bytes()).await?;
                        }
                    }
                }
                _ = flush.tick() => {
                    let out = {
                        let mut e = engine.borrow_mut();
                        render(&e.poll(sid))
                    };
                    if !out.is_empty() {
                        write_half.write_all(out.as_bytes()).await?;
                    }
                }
            }
        }
        Ok(())
    }
    .await;

    // ALWAYS despawn, regardless of clean exit or I/O error.
    {
        let mut e = engine.borrow_mut();
        despawn_player(&mut e, sid, &actor);
    }
    result
}
