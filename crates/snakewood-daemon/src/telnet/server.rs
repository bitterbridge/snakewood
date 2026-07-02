use std::cell::{Cell, RefCell};
use std::rc::Rc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use snakewood_core::{EntityId, Intent};

use crate::telnet::{despawn_player, is_quit, parse, render, spawn_player};
use crate::Engine;

/// Accept connections forever, handling each on a local task.
pub async fn serve(listener: TcpListener, engine: Rc<RefCell<Engine>>, start_room: EntityId) {
    let seq = Rc::new(Cell::new(0u64));
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(_) => continue,
        };
        let n = seq.get();
        seq.set(n + 1);
        let engine = engine.clone();
        let start_room = start_room.clone();
        tokio::task::spawn_local(async move {
            let _ = handle_connection(stream, engine, start_room, n).await;
        });
    }
}

/// Drive one player's connection.
async fn handle_connection(
    stream: TcpStream,
    engine: Rc<RefCell<Engine>>,
    start_room: EntityId,
    seq: u64,
) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    // Spawn the player and greet with a Look (borrow dropped before any await).
    let (sid, actor, greeting) = {
        let mut e = engine.borrow_mut();
        let (sid, actor) = spawn_player(&mut e, &start_room, seq);
        e.submit(
            sid,
            Intent::Look {
                actor: actor.clone(),
            },
        );
        let greeting = render(&e.poll(sid));
        (sid, actor, greeting)
    };
    write_half.write_all(greeting.as_bytes()).await?;

    // Read commands until quit or EOF.
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

    // Clean up: despawn the player.
    {
        let mut e = engine.borrow_mut();
        despawn_player(&mut e, sid, &actor);
    }
    Ok(())
}
