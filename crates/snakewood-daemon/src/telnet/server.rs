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

/// Drive one player's connection.
async fn handle_connection(
    stream: TcpStream,
    engine: Rc<RefCell<Engine>>,
    start_room: EntityId,
) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    // Spawn the player up front so cleanup can always run.
    let (sid, actor) = {
        let mut e = engine.borrow_mut();
        spawn_player(&mut e, &start_room)
    };

    // Run the whole session; capture the result so we can always clean up.
    let result: std::io::Result<()> = async {
        // greeting (Look) — borrow dropped before the write
        let greeting = {
            let mut e = engine.borrow_mut();
            e.submit(
                sid,
                Intent::Look {
                    actor: actor.clone(),
                },
            );
            render(&e.poll(sid))
        };
        write_half.write_all(greeting.as_bytes()).await?;

        loop {
            let line = match lines.next_line().await? {
                Some(line) => line,
                None => break,
            };
            if is_quit(&line) {
                break;
            }
            let reply = {
                let mut e = engine.borrow_mut();
                match parse(&line, &actor) {
                    Some(intent) => {
                        e.submit(sid, intent);
                        render(&e.poll(sid))
                    }
                    None if line.trim().is_empty() => String::new(),
                    None => "What?\r\n".to_string(),
                }
            };
            if !reply.is_empty() {
                write_half.write_all(reply.as_bytes()).await?;
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
