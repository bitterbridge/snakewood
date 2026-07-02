# Snakewood M1 Stage 3c — Telnet Transport Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the daemon a runnable binary you can telnet into — accept TCP connections, spawn a player per connection, translate typed lines into `Intent`s and rendered `PresentationNode`s back to text, all driven against the one shared `Engine`, with a real-socket end-to-end test proving you can connect, `look`, and walk between rooms.

**Architecture:** A **current-thread tokio runtime** hosts everything on one thread, so the sync, `!Send` `Engine` is shared as `Rc<RefCell<Engine>>` across `spawn_local` tasks with no locks and no `Send` bound (cooperative scheduling serializes all world mutation; `RefCell` borrows are always short and never held across `.await`). Pure, unit-tested translation functions convert input lines → `Intent` (`parse`) and `PresentationNode`s → wire text (`render`); sync helpers spawn/despawn a player mob per connection. The async `serve` accept-loop and a tick task wrap those pure pieces; `main` boots an `Engine` from a git `WorldStore` (seeding a starter world if empty) and runs the listener.

**Tech Stack:** Rust (edition 2021), `tokio` (current-thread: `rt`, `net`, `io-util`, `macros`, `time`) added to `snakewood-daemon`, on top of `snakewood-core`/`snakewood-daemon`. `SystemClock` (the one sanctioned wall-clock boundary) for the binary; `ManualClock` in tests.

## Global Constraints

- **Language:** Rust, edition 2021. Telnet lives in `snakewood-daemon` (which becomes a runnable binary). Add `tokio` to `[workspace.dependencies]` and to the daemon's `[dependencies]`; no other new deps (`main` returns `Result<(), Box<dyn std::error::Error>>` — no `anyhow`).
- **Concurrency model:** current-thread tokio runtime + `LocalSet`; `Engine` shared as `Rc<RefCell<Engine>>`; connection handlers and the tick task are `spawn_local`. **Never hold a `RefCell` borrow across an `.await`** — borrow in a tight block, drop it, then do I/O.
- **Wall-clock:** the ONLY place `SystemTime::now()` is allowed is `SystemClock` (the production `Clock` impl, injected). Library logic still takes time via the injected `Clock`. Tests use `ManualClock`.
- **Pure translation:** `parse` (line→`Intent`) and `render` (`&[PresentationNode]`→`String`) are pure functions with no I/O, unit-tested independently of the network.
- **Line protocol:** input is one command per line; unknown/unparseable non-empty input yields a `"What?"` reply (this is the parser layer failing to form an intent, matching the spec). Output lines are joined with `\r\n` (telnet convention).
- **Authorization:** a connection may only drive its own player (already enforced by `Engine::submit`'s session/actor check — the server passes the session's own actor in the `Intent`).
- **`World` FROZEN**; `snakewood-core` unchanged (this stage is daemon-only). `BTreeMap`/`BTreeSet` only.
- **Deferred (do NOT build):** SSH/WS gateways, ANSI color / MXP (render plain readable text for now), authentication/accounts (players are anonymous per-connection), the MCP (Stage 3d), combat/despawn beyond connection cleanup, prompts beyond a minimal one.

---

### Task 1: `tokio` dependency + `SystemClock`

**Files:**
- Modify: `Cargo.toml` (workspace)
- Modify: `crates/snakewood-daemon/Cargo.toml`
- Modify: `crates/snakewood-daemon/src/clock.rs`
- Modify: `crates/snakewood-daemon/src/lib.rs`

**Interfaces:**
- Consumes: existing `Clock` trait.
- Produces:
  - `tokio` available to the daemon (current-thread feature set).
  - `pub struct SystemClock;` implementing `Clock` via `SystemTime::now()` (the sanctioned wall-clock boundary). Re-exported from the daemon crate root.

- [ ] **Step 1: Add `tokio` to `[workspace.dependencies]` in the root `Cargo.toml`**

Add this line under `[workspace.dependencies]`:

```toml
tokio = { version = "1", features = ["rt", "net", "io-util", "macros", "time"] }
```

- [ ] **Step 2: Add `tokio` to the daemon's `[dependencies]`**

In `crates/snakewood-daemon/Cargo.toml`, under `[dependencies]` (keep the existing `snakewood-core` line):

```toml
tokio = { workspace = true }
```

- [ ] **Step 3: Add `SystemClock` to `crates/snakewood-daemon/src/clock.rs` with a test**

Append to `clock.rs`:

```rust
use std::time::{SystemTime, UNIX_EPOCH};

/// The production clock: real wall-clock time. This is the ONLY sanctioned place
/// the daemon reads `SystemTime::now()`; everything else takes time via `Clock`.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod system_clock_tests {
    use super::*;

    #[test]
    fn system_clock_is_after_2020() {
        // 2020-01-01 UTC = 1_577_836_800. A real clock must be well past this.
        assert!(SystemClock.now_unix() > 1_577_836_800);
    }
}
```

- [ ] **Step 4: Re-export `SystemClock` from `lib.rs`**

Change the clock re-export line in `crates/snakewood-daemon/src/lib.rs` to:

```rust
pub use clock::{Clock, ManualClock, SystemClock};
```

- [ ] **Step 5: Build and test**

Run: `cargo test -p snakewood-daemon clock`
Expected: compiles (tokio resolves); `system_clock_is_after_2020` passes plus the existing `manual_clock_starts_at_and_advances`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/snakewood-daemon/Cargo.toml crates/snakewood-daemon/src/clock.rs crates/snakewood-daemon/src/lib.rs
git commit -m "feat: add tokio dependency and SystemClock"
```

---

### Task 2: Input parser (line → `Intent`)

**Files:**
- Create: `crates/snakewood-daemon/src/telnet/mod.rs`
- Create: `crates/snakewood-daemon/src/telnet/parse.rs`
- Modify: `crates/snakewood-daemon/src/lib.rs`

**Interfaces:**
- Consumes: `snakewood_core::{Direction, EntityId, Intent}`.
- Produces:
  - `pub fn parse(line: &str, actor: &EntityId) -> Option<Intent>` — trims and lowercases; maps movement words/abbreviations to `Intent::Move` for `actor`, `look`/`l` to `Intent::Look`; returns `None` for empty or unrecognized input (caller renders `"What?"`).
  - `pub enum ParsedQuit` is NOT used; quitting is handled in the server by recognizing the raw command — but expose `pub fn is_quit(line: &str) -> bool` here (true for `quit`/`q`/`exit`) so the server can detect it. (Quit is a session/transport concern, not a world `Intent`.)

- [ ] **Step 1: Write `crates/snakewood-daemon/src/telnet/parse.rs` with tests**

```rust
use snakewood_core::{Direction, EntityId, Intent};

/// Parse one line of player input into an intent for `actor`.
/// Returns `None` for empty input or an unrecognized command.
pub fn parse(line: &str, actor: &EntityId) -> Option<Intent> {
    let word = line.trim().to_ascii_lowercase();
    let direction = match word.as_str() {
        "n" | "north" => Some(Direction::North),
        "s" | "south" => Some(Direction::South),
        "e" | "east" => Some(Direction::East),
        "w" | "west" => Some(Direction::West),
        "u" | "up" => Some(Direction::Up),
        "d" | "down" => Some(Direction::Down),
        _ => None,
    };
    if let Some(direction) = direction {
        return Some(Intent::Move { actor: actor.clone(), direction });
    }
    match word.as_str() {
        "look" | "l" => Some(Intent::Look { actor: actor.clone() }),
        _ => None,
    }
}

/// Whether a line is a request to disconnect (a transport concern, not an Intent).
pub fn is_quit(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "quit" | "q" | "exit")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> EntityId {
        EntityId::new("player/anon-0").unwrap()
    }

    #[test]
    fn parses_movement_words_and_abbreviations() {
        assert_eq!(parse("north", &actor()), Some(Intent::Move { actor: actor(), direction: Direction::North }));
        assert_eq!(parse("N", &actor()), Some(Intent::Move { actor: actor(), direction: Direction::North }));
        assert_eq!(parse("  d  ", &actor()), Some(Intent::Move { actor: actor(), direction: Direction::Down }));
    }

    #[test]
    fn parses_look() {
        assert_eq!(parse("look", &actor()), Some(Intent::Look { actor: actor() }));
        assert_eq!(parse("l", &actor()), Some(Intent::Look { actor: actor() }));
    }

    #[test]
    fn empty_and_unknown_are_none() {
        assert_eq!(parse("", &actor()), None);
        assert_eq!(parse("   ", &actor()), None);
        assert_eq!(parse("fluffernuts", &actor()), None);
    }

    #[test]
    fn quit_detection() {
        assert!(is_quit("quit"));
        assert!(is_quit(" Q "));
        assert!(is_quit("exit"));
        assert!(!is_quit("look"));
    }
}
```

- [ ] **Step 2: Write `crates/snakewood-daemon/src/telnet/mod.rs`**

```rust
//! The telnet transport: translate a line-oriented text stream to/from the fabric.

pub mod parse;

pub use parse::{is_quit, parse};
```

- [ ] **Step 3: Wire the module into `lib.rs`**

Add to `crates/snakewood-daemon/src/lib.rs`:

```rust
pub mod telnet;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p snakewood-daemon telnet::parse`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-daemon/src/telnet crates/snakewood-daemon/src/lib.rs
git commit -m "feat: add telnet input parser (line -> Intent)"
```

---

### Task 3: Presentation renderer (`PresentationNode`s → text)

**Files:**
- Create: `crates/snakewood-daemon/src/telnet/render.rs`
- Modify: `crates/snakewood-daemon/src/telnet/mod.rs`

**Interfaces:**
- Consumes: `snakewood_core::{Direction, PresentationNode}`.
- Produces:
  - `pub fn render(nodes: &[PresentationNode]) -> String` — one text line per node, joined by `\r\n`, with a trailing `\r\n`. Plain readable text (no ANSI in this stage).
  - Handles every `PresentationNode` variant: `RoomName(s)` → `s`; `RoomDescription(s)` → `s`; `Exits(dirs)` → `"Exits: north, down"` or `"Exits: none"`; `Occupants(names)` → `"Also here: a, b"` (omit the line entirely if empty); `Line(s)` → `s`; `Denied(s)` → `s`; `Prompt` → `">"`.

- [ ] **Step 1: Write `crates/snakewood-daemon/src/telnet/render.rs` with tests**

```rust
use snakewood_core::{Direction, PresentationNode};

fn direction_name(dir: &Direction) -> &'static str {
    match dir {
        Direction::North => "north",
        Direction::South => "south",
        Direction::East => "east",
        Direction::West => "west",
        Direction::Up => "up",
        Direction::Down => "down",
    }
}

fn render_node(node: &PresentationNode) -> Option<String> {
    match node {
        PresentationNode::RoomName(s) => Some(s.clone()),
        PresentationNode::RoomDescription(s) => Some(s.clone()),
        PresentationNode::Exits(dirs) => {
            if dirs.is_empty() {
                Some("Exits: none".to_string())
            } else {
                let names: Vec<&str> = dirs.iter().map(direction_name).collect();
                Some(format!("Exits: {}", names.join(", ")))
            }
        }
        PresentationNode::Occupants(names) => {
            if names.is_empty() {
                None // don't render an empty "Also here:" line
            } else {
                Some(format!("Also here: {}", names.join(", ")))
            }
        }
        PresentationNode::Line(s) => Some(s.clone()),
        PresentationNode::Denied(s) => Some(s.clone()),
        PresentationNode::Prompt => Some(">".to_string()),
    }
}

/// Render a batch of presentation nodes to telnet wire text (CRLF line endings).
pub fn render(nodes: &[PresentationNode]) -> String {
    let mut out = String::new();
    for node in nodes {
        if let Some(line) = render_node(node) {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_room_view() {
        let nodes = vec![
            PresentationNode::RoomName("Snakewood Clearing".to_string()),
            PresentationNode::RoomDescription("A clearing.".to_string()),
            PresentationNode::Exits(vec![Direction::North, Direction::Down]),
            PresentationNode::Occupants(vec!["a goblin".to_string()]),
        ];
        let text = render(&nodes);
        assert_eq!(
            text,
            "Snakewood Clearing\r\nA clearing.\r\nExits: north, down\r\nAlso here: a goblin\r\n"
        );
    }

    #[test]
    fn empty_occupants_line_is_omitted() {
        let nodes = vec![PresentationNode::Occupants(vec![])];
        assert_eq!(render(&nodes), "");
    }

    #[test]
    fn no_exits_says_none() {
        let nodes = vec![PresentationNode::Exits(vec![])];
        assert_eq!(render(&nodes), "Exits: none\r\n");
    }

    #[test]
    fn renders_denied_and_line() {
        let nodes = vec![
            PresentationNode::Denied("You see no exit in that direction.".to_string()),
            PresentationNode::Line("The goblin blocks your way north.".to_string()),
        ];
        assert_eq!(
            render(&nodes),
            "You see no exit in that direction.\r\nThe goblin blocks your way north.\r\n"
        );
    }
}
```

- [ ] **Step 2: Wire into `telnet/mod.rs`**

Add:
```rust
pub mod render;

pub use render::render;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-daemon telnet::render`
Expected: PASS (4 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-daemon/src/telnet/render.rs crates/snakewood-daemon/src/telnet/mod.rs
git commit -m "feat: add telnet presentation renderer (nodes -> text)"
```

---

### Task 4: Player provisioning (spawn/despawn a per-connection player)

**Files:**
- Create: `crates/snakewood-daemon/src/telnet/provision.rs`
- Modify: `crates/snakewood-daemon/src/telnet/mod.rs`

**Interfaces:**
- Consumes: `crate::{Engine, SessionId}`; `snakewood_core::{EntityId, Mob, Flag}`.
- Produces:
  - `pub fn spawn_player(engine: &mut Engine, start_room: &EntityId, seq: u64) -> (SessionId, EntityId)` — inserts a living, conscious anonymous player mob `player/anon-{seq}` at `start_room`, connects a session bound to it, returns both.
  - `pub fn despawn_player(engine: &mut Engine, sid: SessionId, actor: &EntityId)` — disconnects the session and removes the player mob from the realm.

- [ ] **Step 1: Write `crates/snakewood-daemon/src/telnet/provision.rs` with tests**

```rust
use std::collections::BTreeSet;

use snakewood_core::{EntityId, Flag, Mob};

use crate::{Engine, SessionId};

/// Spawn an anonymous player mob at `start_room` and connect a session to it.
pub fn spawn_player(engine: &mut Engine, start_room: &EntityId, seq: u64) -> (SessionId, EntityId) {
    let actor = EntityId::new(format!("player/anon-{seq}")).expect("player id is valid");
    let mut flags = BTreeSet::new();
    flags.insert(Flag::Alive);
    flags.insert(Flag::Conscious);
    engine.realm_mut().insert_mob(Mob {
        id: actor.clone(),
        name: format!("Player{seq}"),
        location: start_room.clone(),
        flags,
        responders: Vec::new(),
    });
    let sid = engine.connect(actor.clone());
    (sid, actor)
}

/// Disconnect a player's session and remove its mob from the world.
pub fn despawn_player(engine: &mut Engine, sid: SessionId, actor: &EntityId) {
    engine.disconnect(sid);
    engine.realm_mut().mobs.remove(actor);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualClock;
    use snakewood_core::{Realm, World};

    #[test]
    fn spawn_places_player_and_binds_session() {
        let mut engine = Engine::new(Realm::new(World::default()), Box::new(ManualClock::new(0)));
        let start = EntityId::new("snakewood/clearing").unwrap();
        let (sid, actor) = spawn_player(&mut engine, &start, 7);
        assert_eq!(actor.as_str(), "player/anon-7");
        assert_eq!(engine.session_actor(sid), Some(&actor));
        assert_eq!(engine.realm().mob_location(&actor).map(|r| r.as_str()), Some("snakewood/clearing"));
    }

    #[test]
    fn despawn_removes_session_and_mob() {
        let mut engine = Engine::new(Realm::new(World::default()), Box::new(ManualClock::new(0)));
        let start = EntityId::new("snakewood/clearing").unwrap();
        let (sid, actor) = spawn_player(&mut engine, &start, 1);
        despawn_player(&mut engine, sid, &actor);
        assert_eq!(engine.session_actor(sid), None);
        assert!(engine.realm().mob(&actor).is_none());
    }
}
```

- [ ] **Step 2: Wire into `telnet/mod.rs`**

Add:
```rust
pub mod provision;

pub use provision::{despawn_player, spawn_player};
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-daemon telnet::provision`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-daemon/src/telnet/provision.rs crates/snakewood-daemon/src/telnet/mod.rs
git commit -m "feat: add per-connection player spawn/despawn provisioning"
```

---

### Task 5: The async telnet server (accept loop + connection handler)

**Files:**
- Create: `crates/snakewood-daemon/src/telnet/server.rs`
- Modify: `crates/snakewood-daemon/src/telnet/mod.rs`

**Interfaces:**
- Consumes: `Rc<RefCell<Engine>>`; `parse`/`is_quit`/`render`/`spawn_player`/`despawn_player`; `tokio::net::{TcpListener, TcpStream}`; `snakewood_core::{EntityId, Intent}`.
- Produces:
  - `pub async fn serve(listener: TcpListener, engine: Rc<RefCell<Engine>>, start_room: EntityId)` — accept loop; each accepted connection is `spawn_local`'d into `handle_connection`. A shared `Rc<Cell<u64>>` mints the per-connection sequence number.
  - `async fn handle_connection(stream: TcpStream, engine: Rc<RefCell<Engine>>, start_room: EntityId, seq: u64)` — spawns a player, greets with a `Look`, then loops reading lines: `is_quit` → break; `parse` → `Some(intent)` → submit+poll+render+write; `None` (non-empty) → write `"What?"`; EOF → break. On exit, despawns the player.

- [ ] **Step 1: Write `crates/snakewood-daemon/src/telnet/server.rs`**

```rust
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

    // Spawn the player and greet with a Look (borrow is dropped before any await).
    let (sid, actor) = {
        let mut e = engine.borrow_mut();
        let (sid, actor) = spawn_player(&mut e, &start_room, seq);
        e.submit(sid, Intent::Look { actor: actor.clone() });
        let nodes = e.poll(sid);
        (sid, actor, render(&nodes))
    }
    .into_greeting();

    write_half.write_all(sid.as_bytes()).await?;

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
            match parse(&line, &actor.actor) {
                Some(intent) => {
                    e.submit(actor.sid, intent);
                    render(&e.poll(actor.sid))
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
        despawn_player(&mut e, actor.sid, &actor.actor);
    }
    Ok(())
}
```

> NOTE TO IMPLEMENTER: the `.into_greeting()` / `sid.as_bytes()` / `actor.sid` / `actor.actor` fragments above are DELIBERATELY WRONG placeholders to force you to bind the tuple cleanly — the block returns `(SessionId, EntityId, String)`. Replace that section with the straightforward, correct binding below and use it verbatim:
>
> ```rust
>     // Spawn the player and greet with a Look (borrow dropped before any await).
>     let (sid, actor, greeting) = {
>         let mut e = engine.borrow_mut();
>         let (sid, actor) = spawn_player(&mut e, &start_room, seq);
>         e.submit(sid, Intent::Look { actor: actor.clone() });
>         let greeting = render(&e.poll(sid));
>         (sid, actor, greeting)
>     };
>     write_half.write_all(greeting.as_bytes()).await?;
>
>     loop {
>         let line = match lines.next_line().await? {
>             Some(line) => line,
>             None => break,
>         };
>         if is_quit(&line) {
>             break;
>         }
>         let reply = {
>             let mut e = engine.borrow_mut();
>             match parse(&line, &actor) {
>                 Some(intent) => {
>                     e.submit(sid, intent);
>                     render(&e.poll(sid))
>                 }
>                 None if line.trim().is_empty() => String::new(),
>                 None => "What?\r\n".to_string(),
>             }
>         };
>         if !reply.is_empty() {
>             write_half.write_all(reply.as_bytes()).await?;
>         }
>     }
>
>     {
>         let mut e = engine.borrow_mut();
>         despawn_player(&mut e, sid, &actor);
>     }
>     Ok(())
> ```
>
> Write ONLY this correct version in the file. Do not include the placeholder fragments. Confirm no `RefCell` borrow is held across an `.await` (each `{ ... }` block drops its borrow before the following `write_all(...).await`).

- [ ] **Step 2: Wire into `telnet/mod.rs`**

Add:
```rust
pub mod server;

pub use server::serve;
```

- [ ] **Step 3: Build (no unit test here — exercised by the e2e in Task 7)**

Run: `cargo build -p snakewood-daemon`
Expected: compiles cleanly. Run `cargo clippy -p snakewood-daemon --all-targets -- -D warnings` and fix any warnings (e.g. an unused import).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-daemon/src/telnet/server.rs crates/snakewood-daemon/src/telnet/mod.rs
git commit -m "feat: add async telnet accept loop and connection handler"
```

---

### Task 6: Tick task + `main` binary

**Files:**
- Create: `crates/snakewood-daemon/src/telnet/tick.rs`
- Create: `crates/snakewood-daemon/src/main.rs`
- Modify: `crates/snakewood-daemon/src/telnet/mod.rs`

**Interfaces:**
- Consumes: `Rc<RefCell<Engine>>`; `serve`; `GitStore`/`SystemClock`; `tokio::time`.
- Produces:
  - `pub async fn run_tick_loop(engine: Rc<RefCell<Engine>>, period_secs: u64)` — every `period_secs` wall seconds, borrow the engine, `tick()`, and `maybe_snapshot()` (ignoring a snapshot error after logging to stderr).
  - `crates/snakewood-daemon/src/main.rs` — a current-thread runtime that boots an `Engine` from a git data dir (default `./snakewood-data`, override via `SNAKEWOOD_DATA`), seeds a 2-room starter world if the store is empty, sets an hourly snapshot interval, binds `127.0.0.1:4000` (override via `SNAKEWOOD_ADDR`), and runs `serve` + `run_tick_loop` on a `LocalSet`.

- [ ] **Step 1: Write `crates/snakewood-daemon/src/telnet/tick.rs`**

```rust
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::Engine;

/// Advance the world once per `period_secs` and take interval snapshots.
pub async fn run_tick_loop(engine: Rc<RefCell<Engine>>, period_secs: u64) {
    let mut interval = tokio::time::interval(Duration::from_secs(period_secs));
    loop {
        interval.tick().await;
        let mut e = engine.borrow_mut();
        e.tick();
        if let Err(err) = e.maybe_snapshot() {
            eprintln!("snapshot failed: {err:?}");
        }
    }
}
```

- [ ] **Step 2: Wire `tick` into `telnet/mod.rs`**

Add:
```rust
pub mod tick;

pub use tick::run_tick_loop;
```

- [ ] **Step 3: Write `crates/snakewood-daemon/src/main.rs`**

```rust
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use snakewood_core::{Direction, EntityId, GitStore, Realm, Room, World};
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
    engine.checkpoint("seed starter world")?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var("SNAKEWOOD_DATA").unwrap_or_else(|_| "./snakewood-data".to_string());
    let addr = std::env::var("SNAKEWOOD_ADDR").unwrap_or_else(|_| "127.0.0.1:4000".to_string());

    let store = GitStore::init(&data_dir)?;
    let mut engine = Engine::boot(Box::new(store), Box::new(SystemClock))?;
    seed_if_empty(&mut engine)?;
    engine.set_snapshot_interval(3600);
    let engine = Rc::new(RefCell::new(engine));
    let start_room = id("snakewood/clearing");

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
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
```

- [ ] **Step 4: Build**

Run: `cargo build -p snakewood-daemon`
Expected: compiles (both the lib and the `snakewood-daemon` binary). Run `cargo clippy -p snakewood-daemon --all-targets -- -D warnings` and fix any warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-daemon/src/telnet/tick.rs crates/snakewood-daemon/src/main.rs crates/snakewood-daemon/src/telnet/mod.rs
git commit -m "feat: add tick loop and runnable telnet daemon binary"
```

---

### Task 7: End-to-end test — connect over TCP and walk around

**Files:**
- Create: `crates/snakewood-daemon/tests/telnet_e2e.rs`

**Interfaces:**
- Consumes: public API — `snakewood_daemon::{Engine, ManualClock}`, `snakewood_daemon::telnet::serve`; `snakewood_core::{Realm, World, Room, Direction, EntityId}`; `tokio`.
- Produces: an integration test that binds an ephemeral port, runs `serve` on a `LocalSet`, connects a real `TcpStream`, and asserts the greeting shows the start room, `n` moves to the next room, and an unknown command returns `"What?"`.

- [ ] **Step 1: Write `crates/snakewood-daemon/tests/telnet_e2e.rs`**

```rust
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use snakewood_core::{Direction, EntityId, Realm, Room, World};
use snakewood_daemon::telnet::serve;
use snakewood_daemon::{Engine, ManualClock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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

/// Read whatever arrives within `ms` into a string (best-effort).
async fn read_for(stream: &mut TcpStream, ms: u64) -> String {
    let mut buf = vec![0u8; 4096];
    let mut acc = String::new();
    loop {
        match tokio::time::timeout(Duration::from_millis(ms), stream.read(&mut buf)).await {
            Ok(Ok(0)) => break,          // EOF
            Ok(Ok(n)) => acc.push_str(&String::from_utf8_lossy(&buf[..n])),
            Ok(Err(_)) => break,         // read error
            Err(_) => break,             // timeout: assume no more for now
        }
    }
    acc
}

#[test]
fn connect_look_and_walk() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let engine = Rc::new(RefCell::new(two_room_engine()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::task::spawn_local(serve(listener, engine, id("snakewood/clearing")));

        let mut client = TcpStream::connect(addr).await.unwrap();

        // Greeting shows the start room.
        let greeting = read_for(&mut client, 300).await;
        assert!(greeting.contains("Snakewood Clearing"), "greeting was: {greeting:?}");

        // Walk north -> arrive at the Old Well.
        client.write_all(b"n\r\n").await.unwrap();
        let after_move = read_for(&mut client, 300).await;
        assert!(after_move.contains("The Old Well"), "after_move was: {after_move:?}");

        // Unknown command -> What?
        client.write_all(b"fluffernuts\r\n").await.unwrap();
        let after_unknown = read_for(&mut client, 300).await;
        assert!(after_unknown.contains("What?"), "after_unknown was: {after_unknown:?}");
    });
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p snakewood-daemon --test telnet_e2e`
Expected: PASS (`connect_look_and_walk`). If it hangs, the most likely cause is a `RefCell` borrow held across an `.await` in `server.rs` (a panic "already borrowed" or a deadlock) — fix the server, do NOT loosen the test timeouts beyond a couple hundred ms.

- [ ] **Step 3: Commit**

```bash
git add crates/snakewood-daemon/tests/telnet_e2e.rs
git commit -m "test: end-to-end telnet connect, look, and walk between rooms"
```

---

### Task 8: Stage-completion verification (incl. manual smoke)

**Files:** none (verification only).

- [ ] **Step 1: Full workspace test suite**

Run: `cargo test --workspace`
Expected: all pass — `snakewood-core` (unchanged) and `snakewood-daemon` (clock incl. SystemClock, telnet parse/render/provision, engine, integration tests incl. `telnet_e2e`).

- [ ] **Step 2: Clippy and formatting**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no diffs. If `cargo fmt --check` reports diffs, run `cargo fmt`, re-run the suite, commit.

- [ ] **Step 3: Manual smoke (optional but recommended) — actually connect**

Run the daemon in the background against a temp data dir and drive it with a raw TCP client:

```bash
SNAKEWOOD_DATA="$(mktemp -d)/world" SNAKEWOOD_ADDR=127.0.0.1:4055 cargo run -p snakewood-daemon &
sleep 3
printf 'look\r\nn\r\nfluffernuts\r\nquit\r\n' | nc -w2 127.0.0.1 4055 || true
kill %1 2>/dev/null || true
```
Expected: output shows "Snakewood Clearing", then "The Old Well" after `n`, then "What?" for the unknown command. (This is a sanity check; the automated proof is `telnet_e2e`. If `nc` is unavailable, skip.)

- [ ] **Step 4: Commit any formatting fixes**

```bash
git add -A
git commit -m "chore: stage 3c verification — clippy clean, cargo fmt, workspace green"
```
(Skip if nothing to commit.)

---

## Self-Review

**1. Spec coverage (spec §2 telnet-as-LCD front door; §7 M1 telnet slice):**
- Telnet listener; connect/look/move over a line protocol — Tasks 5–7. ✓
- Presentation rendered to text at the edge (core stays semantic) — Task 3. ✓
- Input parsed to `Intent`; unknown → "What?" at the parser layer — Tasks 2, 5. ✓
- Daemon is a runnable binary that boots from the git store (and seeds if empty) — Task 6. ✓
- Live world persists (hourly snapshot + boot) — reuses Stage 3b-scheduler; wired in `main` — Task 6. ✓
- Correctly deferred: SSH/WS gateways, ANSI/MXP color, auth/accounts, MCP (3d). ✓

**2. Placeholder scan:** No "TBD/TODO". Task 5 Step 1 contains a DELIBERATE, clearly-labeled wrong-placeholder fragment immediately followed by the correct verbatim block and an explicit instruction to write only the correct version — this is a guardrail against the borrow-across-await footgun, not an unfinished placeholder. The manual smoke (Task 8 Step 3) is explicitly optional.

**3. Type consistency:** `parse(&str, &EntityId) -> Option<Intent>`, `is_quit(&str) -> bool`, `render(&[PresentationNode]) -> String`, `spawn_player(&mut Engine, &EntityId, u64) -> (SessionId, EntityId)`, `despawn_player(&mut Engine, SessionId, &EntityId)`, `serve(TcpListener, Rc<RefCell<Engine>>, EntityId)`, `run_tick_loop(Rc<RefCell<Engine>>, u64)` used consistently across Tasks 2–7. `SystemClock` implements `Clock` (Task 1). `main` uses `Engine::boot`/`checkpoint`/`set_snapshot_interval` (Stage 3b-scheduler) and `Realm.world.insert_room`/`realm_mut` (Stage 1/2). The e2e uses only public API (`snakewood_daemon::telnet::serve`, `Engine::new`, `ManualClock`). `PresentationNode` variants matched exhaustively in `render` (Task 3) match Stage 2's definition (`RoomName/RoomDescription/Exits/Occupants/Line/Denied/Prompt`). All borrows in `server.rs`/`tick.rs` are scoped blocks dropped before any `.await`. ✓
