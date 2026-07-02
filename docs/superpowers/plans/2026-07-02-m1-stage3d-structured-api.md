# Snakewood M1 Stage 3d — Structured Command API + `dig` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose a second daemon listener speaking newline-delimited JSON — the structured command API the MCP will tap — with IC `look`/`move` returning structured `PresentationNode`s (not rendered text) and the first OOC world-building op `dig` (create+link a room, then `checkpoint`), all testable over a plain JSON socket.

**Architecture:** `PresentationNode` gains serde so it serializes onto the wire (the design's "structured observation"). `Direction::opposite()` supports two-way exit linking. A new `Engine::dig` mutates the world (insert a room, link the current room's exit and the new room's back-exit) and `checkpoint`s — the first user of the on-change persistence policy. A pure `handle_api_request(&mut Engine, ApiRequest, ...) -> ApiResponse` dispatcher maps `Connect`/`Look`/`Move`/`Dig`/`Disconnect` to Engine ops; an async `serve_api` accept loop wraps it (same current-thread + `Rc<RefCell<Engine>>` model as telnet), and `main` runs telnet + API + tick concurrently via `tokio::join!`.

**Tech Stack:** Rust (edition 2021), `serde` + `serde_json` (JSON wire protocol) added to `snakewood-daemon`; `snakewood-core` gains a serde derive on `PresentationNode` and a `Direction::opposite()`; tokio current-thread (as in Stage 3c).

## Global Constraints

- **Language:** Rust, edition 2021. New work in `snakewood-daemon` + two small additive `snakewood-core` changes (serde on `PresentationNode`; `Direction::opposite`). Add `serde_json` to `[workspace.dependencies]`; add `serde` + `serde_json` to the daemon's `[dependencies]`. No other new deps.
- **Wire protocol:** newline-delimited JSON (one JSON object per line) over a TCP socket on `127.0.0.1`, port from `SNAKEWOOD_API_ADDR` (default `127.0.0.1:4001`) — distinct from telnet's `SNAKEWOOD_ADDR` (4000).
- **Structured, not rendered:** the API returns `PresentationNode`s as JSON (the client renders/interprets), NOT telnet's plain text. This is the "IC returns structured observations" seam.
- **Concurrency:** same as Stage 3c — current-thread tokio + `Rc<RefCell<Engine>>` + `spawn_local`; **no `RefCell` borrow across any `.await`**. `main` runs telnet `serve` + `serve_api` concurrently with `tokio::join!` (both accept loops run forever), tick via `spawn_local`.
- **`dig` is OOC + authored:** it mutates the world directly (not a fabric `Intent`) and calls `Engine::checkpoint` (the on-change policy's first real user). A connection may only `dig` relative to its own player's current room (the session→actor binding).
- **`World` behavior otherwise unchanged**; `BTreeMap`/`BTreeSet` only. `Event` serde is NOT added (the API exposes presentation, not events, this stage).
- **Deferred (do NOT build):** the MCP-protocol bridge itself (Stage 3d-mcp — a separate binary speaking MCP stdio to the socket), auth/accounts, `dig` validation beyond "id is valid + room doesn't already exist", deleting/renaming rooms.

---

### Task 1: serde on `PresentationNode`

**Files:**
- Modify: `crates/snakewood-core/src/presentation.rs`

**Interfaces:**
- Consumes: `Direction` (already serde).
- Produces: `PresentationNode` derives `Serialize, Deserialize` (in addition to `Debug, Clone, PartialEq`), so it round-trips on the JSON wire.

- [ ] **Step 1: Add the derives and a round-trip test**

In `crates/snakewood-core/src/presentation.rs`, add the serde import and extend the derive:

```rust
use serde::{Deserialize, Serialize};

use crate::Direction;

/// A semantic unit of output. Transports render these (telnet) or pass them as
/// structured data (the command API); the core never emits formatted text.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum PresentationNode {
    // ... variants unchanged ...
}
```

Add a test module (the crate already depends on `ron`, so round-trip via RON — no new dep needed):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_node_round_trips_via_serde() {
        let node = PresentationNode::Exits(vec![Direction::North, Direction::Down]);
        let text = ron::ser::to_string(&node).unwrap();
        let back: PresentationNode = ron::from_str(&text).unwrap();
        assert_eq!(back, node);

        let line = PresentationNode::Line("hello".to_string());
        let back2: PresentationNode = ron::from_str(&ron::ser::to_string(&line).unwrap()).unwrap();
        assert_eq!(back2, line);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p snakewood-core presentation`
Expected: PASS (`presentation_node_round_trips_via_serde`). Then `cargo test -p snakewood-core` (no regressions — Stage 1 golden/round-trip still green; `PresentationNode` isn't part of `World` serialization).

- [ ] **Step 3: Commit**

```bash
git add crates/snakewood-core/src/presentation.rs
git commit -m "feat: derive serde on PresentationNode for the wire protocol"
```

---

### Task 2: `Direction::opposite()`

**Files:**
- Modify: `crates/snakewood-core/src/direction.rs`

**Interfaces:**
- Produces: `Direction::opposite(&self) -> Direction` (North↔South, East↔West, Up↔Down).

- [ ] **Step 1: Add the method + test**

Add an `impl Direction` block (above or below the `#[cfg(test)]` module) in `crates/snakewood-core/src/direction.rs`:

```rust
impl Direction {
    /// The reverse direction (for linking exits both ways).
    pub fn opposite(&self) -> Direction {
        match self {
            Direction::North => Direction::South,
            Direction::South => Direction::North,
            Direction::East => Direction::West,
            Direction::West => Direction::East,
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
        }
    }
}
```

Add to the existing `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn opposite_pairs() {
        assert_eq!(Direction::North.opposite(), Direction::South);
        assert_eq!(Direction::South.opposite(), Direction::North);
        assert_eq!(Direction::East.opposite(), Direction::West);
        assert_eq!(Direction::West.opposite(), Direction::East);
        assert_eq!(Direction::Up.opposite(), Direction::Down);
        assert_eq!(Direction::Down.opposite(), Direction::Up);
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p snakewood-core direction`
Expected: PASS (`opposite_pairs` + existing direction tests).

- [ ] **Step 3: Commit**

```bash
git add crates/snakewood-core/src/direction.rs
git commit -m "feat: add Direction::opposite for two-way exit linking"
```

---

### Task 3: `Engine::dig` (OOC world-building + checkpoint)

**Files:**
- Modify: `crates/snakewood-daemon/src/engine.rs`

**Interfaces:**
- Consumes: `Engine` (session registry, `realm_mut`, `checkpoint`); `snakewood_core::{Direction, EntityId, Room, StoreError, Room}`.
- Produces:
  - `pub enum DigError { NoSession, NoLocation, InvalidId(String), RoomExists, Store(StoreError) }` (derive `Debug`).
  - `Engine::dig(&mut self, session: SessionId, direction: Direction, new_id: &str, name: &str, description: &str) -> Result<EntityId, DigError>` — resolves the session's actor and its current room, creates a new room `new_id` (must be a valid `EntityId` and not already exist) with a back-exit `direction.opposite() -> current_room`, links `current_room.exits[direction] = new_id`, then `checkpoint`s. Returns the new room's id.

- [ ] **Step 1: Write the failing test (add to `engine.rs` tests)**

```rust
    #[test]
    fn dig_creates_linked_room_and_persists() {
        use snakewood_core::GitStore;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let new_room_str = "snakewood/hollow";
        {
            let mut realm = Realm::new(world_two_rooms()); // clearing --north--> old-well
            let mut flags = std::collections::BTreeSet::new();
            flags.insert(snakewood_core::Flag::Alive);
            realm.insert_mob(snakewood_core::Mob {
                id: EntityId::new("snakewood/pc/nathan").unwrap(),
                name: "Nathan".to_string(),
                location: EntityId::new("snakewood/clearing").unwrap(),
                flags,
                responders: Vec::new(),
            });
            let store = GitStore::init(dir.path()).unwrap();
            let mut e = Engine::new(realm, Box::new(ManualClock::new(1000)));
            e.attach_store(Box::new(store));
            let sid = e.connect(EntityId::new("snakewood/pc/nathan").unwrap());
            // Dig east from the clearing into a new hollow.
            let created = e
                .dig(sid, Direction::East, new_room_str, "A Hollow", "A mossy hollow.")
                .unwrap();
            assert_eq!(created.as_str(), new_room_str);
            // The new room exists with a back-exit west to the clearing.
            let hollow = e.realm().world.room(&EntityId::new(new_room_str).unwrap()).unwrap();
            assert_eq!(hollow.exits.get(&Direction::West).map(|r| r.as_str()), Some("snakewood/clearing"));
            // The clearing now has an east exit to the hollow.
            let clearing = e.realm().world.room(&EntityId::new("snakewood/clearing").unwrap()).unwrap();
            assert_eq!(clearing.exits.get(&Direction::East).map(|r| r.as_str()), Some(new_room_str));
        }
        // Persisted: a fresh boot from the same dir has the dug room.
        let store = GitStore::init(dir.path()).unwrap();
        let e2 = Engine::boot(Box::new(store), Box::new(ManualClock::new(2000))).unwrap();
        assert!(e2.realm().world.room(&EntityId::new(new_room_str).unwrap()).is_some());
    }

    #[test]
    fn dig_rejects_existing_room() {
        let mut realm = Realm::new(world_two_rooms());
        let mut flags = std::collections::BTreeSet::new();
        flags.insert(snakewood_core::Flag::Alive);
        realm.insert_mob(snakewood_core::Mob {
            id: EntityId::new("snakewood/pc/nathan").unwrap(),
            name: "Nathan".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        });
        let mut e = Engine::new(realm, Box::new(ManualClock::new(0)));
        let sid = e.connect(EntityId::new("snakewood/pc/nathan").unwrap());
        // old-well already exists -> RoomExists.
        let result = e.dig(sid, Direction::East, "snakewood/old-well", "dup", "dup");
        assert!(matches!(result, Err(DigError::RoomExists)));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p snakewood-daemon engine::tests::dig_creates_linked_room_and_persists`
Expected: FAIL to compile — `dig`/`DigError` don't exist.

- [ ] **Step 3: Implement `DigError` and `Engine::dig`**

Add the import (merge into `engine.rs`'s `use snakewood_core::{...}` line): add `Direction, Room`. Then:

```rust
/// Why a `dig` failed.
#[derive(Debug)]
pub enum DigError {
    NoSession,
    NoLocation,
    InvalidId(String),
    RoomExists,
    Store(StoreError),
}
```

Add to the `impl Engine` block:

```rust
    /// OOC world-building: create a new room reached by `direction` from the
    /// session's current room, linked both ways, and checkpoint it.
    pub fn dig(
        &mut self,
        session: SessionId,
        direction: Direction,
        new_id: &str,
        name: &str,
        description: &str,
    ) -> Result<EntityId, DigError> {
        let actor = self.session_actor(session).ok_or(DigError::NoSession)?.clone();
        let current = self
            .realm()
            .mob_location(&actor)
            .ok_or(DigError::NoLocation)?
            .clone();
        let new_room_id = EntityId::new(new_id).map_err(|_| DigError::InvalidId(new_id.to_string()))?;
        if self.realm().world.room(&new_room_id).is_some() {
            return Err(DigError::RoomExists);
        }
        // Create the new room with a back-exit to the current room.
        let mut exits = std::collections::BTreeMap::new();
        exits.insert(direction.opposite(), current.clone());
        self.realm_mut().world.insert_room(Room {
            id: new_room_id.clone(),
            name: name.to_string(),
            description: description.to_string(),
            exits,
        });
        // Link the current room's exit to the new room.
        if let Some(room) = self.realm_mut().world.rooms.get_mut(&current) {
            room.exits.insert(direction, new_room_id.clone());
        }
        self.checkpoint(&format!("dig {new_id}")).map_err(DigError::Store)?;
        Ok(new_room_id)
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p snakewood-daemon engine`
Expected: PASS (`dig_creates_linked_room_and_persists`, `dig_rejects_existing_room`, plus all prior engine tests). Note: `dig_creates_linked_room_and_persists` proves the on-change checkpoint persisted the dug room across a fresh boot.

- [ ] **Step 5: fmt + clippy, then commit**

Run: `cargo fmt -p snakewood-daemon && cargo fmt -p snakewood-daemon -- --check && cargo clippy -p snakewood-daemon --all-targets -- -D warnings`
Then:
```bash
git add crates/snakewood-daemon/src/engine.rs
git commit -m "feat: add Engine::dig (OOC world-building with checkpoint)"
```

---

### Task 4: API protocol types (`ApiRequest` / `ApiResponse`)

**Files:**
- Modify: `Cargo.toml` (workspace) — add `serde_json`
- Modify: `crates/snakewood-daemon/Cargo.toml` — add `serde` + `serde_json`
- Create: `crates/snakewood-daemon/src/api/mod.rs`
- Create: `crates/snakewood-daemon/src/api/protocol.rs`
- Modify: `crates/snakewood-daemon/src/lib.rs`

**Interfaces:**
- Consumes: `snakewood_core::{Direction, PresentationNode}` (PresentationNode now serde, Task 1).
- Produces:
  - `pub enum ApiRequest` (serde, `#[serde(tag = "op", rename_all = "snake_case")]`): `Connect`, `Look { session: u64 }`, `Move { session: u64, direction: Direction }`, `Dig { session: u64, direction: Direction, id: String, name: String, description: String }`, `Disconnect { session: u64 }`.
  - `pub enum ApiResponse` (serde, `#[serde(tag = "status", rename_all = "snake_case")]`): `Connected { session: u64, actor: String, view: Vec<PresentationNode> }`, `Ok { messages: Vec<PresentationNode> }`, `Error { message: String }`.
  - The `api` module, re-exported from the crate root.

- [ ] **Step 1: Add deps**

In the root `Cargo.toml` `[workspace.dependencies]`:
```toml
serde_json = "1"
```
In `crates/snakewood-daemon/Cargo.toml` `[dependencies]` (keep `snakewood-core`, `tokio`):
```toml
serde = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 2: Write `crates/snakewood-daemon/src/api/protocol.rs` with tests**

```rust
use serde::{Deserialize, Serialize};

use snakewood_core::{Direction, PresentationNode};

/// A structured command from an API client (e.g. the MCP bridge).
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ApiRequest {
    Connect,
    Look { session: u64 },
    Move { session: u64, direction: Direction },
    Dig {
        session: u64,
        direction: Direction,
        id: String,
        name: String,
        description: String,
    },
    Disconnect { session: u64 },
}

/// A structured response to an API client.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApiResponse {
    Connected {
        session: u64,
        actor: String,
        view: Vec<PresentationNode>,
    },
    Ok {
        messages: Vec<PresentationNode>,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_json_round_trips() {
        let req = ApiRequest::Move { session: 3, direction: Direction::North };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"op\":\"move\""));
        let back: ApiRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn dig_request_parses_from_json() {
        let json = r#"{"op":"dig","session":1,"direction":"East","id":"snakewood/hollow","name":"A Hollow","description":"Mossy."}"#;
        let req: ApiRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            req,
            ApiRequest::Dig {
                session: 1,
                direction: Direction::East,
                id: "snakewood/hollow".to_string(),
                name: "A Hollow".to_string(),
                description: "Mossy.".to_string(),
            }
        );
    }

    #[test]
    fn response_json_round_trips() {
        let resp = ApiResponse::Ok {
            messages: vec![PresentationNode::Line("hi".to_string())],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"status\":\"ok\""));
        let back: ApiResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, resp);
    }
}
```

- [ ] **Step 3: Write `crates/snakewood-daemon/src/api/mod.rs`**

```rust
//! The structured command API: newline-delimited JSON in/out (the MCP bridge taps this).

pub mod protocol;

pub use protocol::{ApiRequest, ApiResponse};
```

- [ ] **Step 4: Wire into `lib.rs`**

Add to `crates/snakewood-daemon/src/lib.rs`:
```rust
pub mod api;
```

- [ ] **Step 5: Run tests + fmt + clippy**

Run: `cargo test -p snakewood-daemon api::protocol` (3 tests pass), then `cargo fmt -p snakewood-daemon && cargo fmt -p snakewood-daemon -- --check && cargo clippy -p snakewood-daemon --all-targets -- -D warnings`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/snakewood-daemon/Cargo.toml crates/snakewood-daemon/src/api crates/snakewood-daemon/src/lib.rs
git commit -m "feat: add structured API protocol types (ApiRequest/ApiResponse)"
```

---

### Task 5: `handle_api_request` — the sync dispatcher

**Files:**
- Create: `crates/snakewood-daemon/src/api/handler.rs`
- Modify: `crates/snakewood-daemon/src/api/mod.rs`

**Interfaces:**
- Consumes: `Engine`, `SessionId`, `ApiRequest`/`ApiResponse`; `telnet::spawn_player`/`despawn_player`; `snakewood_core::{EntityId, Intent}`.
- Produces:
  - `pub fn handle_api_request(engine: &mut Engine, req: ApiRequest, start_room: &EntityId, next_player: &mut u64) -> ApiResponse` — `Connect` spawns a player (incrementing `next_player`), submits a `Look`, returns `Connected { session, actor, view }`; `Look`/`Move` submit the intent for the session's bound actor and return `Ok { messages }` (or `Error` if the session/actor is unknown); `Dig` calls `Engine::dig` then returns a `Look` view (or `Error` on `DigError`); `Disconnect` despawns and returns `Ok { messages: [] }`.

- [ ] **Step 1: Write `crates/snakewood-daemon/src/api/handler.rs` with tests**

```rust
use snakewood_core::{EntityId, Intent};

use crate::api::{ApiRequest, ApiResponse};
use crate::telnet::{despawn_player, spawn_player};
use crate::{Engine, SessionId};

/// Look up the actor bound to a session, or produce an Error response.
fn actor_of(engine: &Engine, session: u64) -> Result<EntityId, ApiResponse> {
    match engine.session_actor(SessionId(session)) {
        Some(actor) => Ok(actor.clone()),
        None => Err(ApiResponse::Error { message: format!("unknown session {session}") }),
    }
}

/// Dispatch a structured API request against the engine.
pub fn handle_api_request(
    engine: &mut Engine,
    req: ApiRequest,
    start_room: &EntityId,
    next_player: &mut u64,
) -> ApiResponse {
    match req {
        ApiRequest::Connect => {
            let seq = *next_player;
            *next_player += 1;
            let (sid, actor) = spawn_player(engine, start_room, seq);
            engine.submit(sid, Intent::Look { actor: actor.clone() });
            let view = engine.poll(sid);
            ApiResponse::Connected { session: sid.0, actor: actor.to_string(), view }
        }
        ApiRequest::Look { session } => {
            let actor = match actor_of(engine, session) {
                Ok(a) => a,
                Err(e) => return e,
            };
            engine.submit(SessionId(session), Intent::Look { actor });
            ApiResponse::Ok { messages: engine.poll(SessionId(session)) }
        }
        ApiRequest::Move { session, direction } => {
            let actor = match actor_of(engine, session) {
                Ok(a) => a,
                Err(e) => return e,
            };
            engine.submit(SessionId(session), Intent::Move { actor, direction });
            ApiResponse::Ok { messages: engine.poll(SessionId(session)) }
        }
        ApiRequest::Dig { session, direction, id, name, description } => {
            match engine.dig(SessionId(session), direction, &id, &name, &description) {
                Ok(_) => {
                    // Show the updated room so the client sees the new exit.
                    if let Some(actor) = engine.session_actor(SessionId(session)).cloned() {
                        engine.submit(SessionId(session), Intent::Look { actor });
                        ApiResponse::Ok { messages: engine.poll(SessionId(session)) }
                    } else {
                        ApiResponse::Ok { messages: Vec::new() }
                    }
                }
                Err(e) => ApiResponse::Error { message: format!("dig failed: {e:?}") },
            }
        }
        ApiRequest::Disconnect { session } => {
            if let Some(actor) = engine.session_actor(SessionId(session)).cloned() {
                despawn_player(engine, SessionId(session), &actor);
            }
            ApiResponse::Ok { messages: Vec::new() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualClock;
    use snakewood_core::{Direction, PresentationNode, Realm};

    // Reuse the engine test helper's two-room world by rebuilding it here.
    fn engine() -> Engine {
        use snakewood_core::{Room, World};
        use std::collections::BTreeMap;
        let mut exits = BTreeMap::new();
        exits.insert(Direction::North, EntityId::new("snakewood/old-well").unwrap());
        let mut world = World::default();
        world.insert_room(Room {
            id: EntityId::new("snakewood/clearing").unwrap(),
            name: "Snakewood Clearing".to_string(),
            description: "A clearing.".to_string(),
            exits,
        });
        world.insert_room(Room {
            id: EntityId::new("snakewood/old-well").unwrap(),
            name: "The Old Well".to_string(),
            description: "A well.".to_string(),
            exits: BTreeMap::new(),
        });
        Engine::new(Realm::new(world), Box::new(ManualClock::new(0)))
    }

    fn start() -> EntityId {
        EntityId::new("snakewood/clearing").unwrap()
    }

    #[test]
    fn connect_returns_session_and_start_room_view() {
        let mut e = engine();
        let mut seq = 0;
        let resp = handle_api_request(&mut e, ApiRequest::Connect, &start(), &mut seq);
        match resp {
            ApiResponse::Connected { session, actor, view } => {
                assert_eq!(actor, "player/anon-0");
                assert_eq!(session, 0);
                assert!(view.contains(&PresentationNode::RoomName("Snakewood Clearing".to_string())));
            }
            other => panic!("expected Connected, got {other:?}"),
        }
        assert_eq!(seq, 1);
    }

    #[test]
    fn move_returns_new_room_view() {
        let mut e = engine();
        let mut seq = 0;
        let ApiResponse::Connected { session, .. } =
            handle_api_request(&mut e, ApiRequest::Connect, &start(), &mut seq)
        else {
            panic!("connect failed");
        };
        let resp = handle_api_request(&mut e, ApiRequest::Move { session, direction: Direction::North }, &start(), &mut seq);
        match resp {
            ApiResponse::Ok { messages } => {
                assert!(messages.contains(&PresentationNode::RoomName("The Old Well".to_string())));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn dig_then_look_shows_new_exit() {
        let mut e = engine();
        let mut seq = 0;
        let ApiResponse::Connected { session, .. } =
            handle_api_request(&mut e, ApiRequest::Connect, &start(), &mut seq)
        else {
            panic!("connect failed");
        };
        let resp = handle_api_request(
            &mut e,
            ApiRequest::Dig {
                session,
                direction: Direction::East,
                id: "snakewood/hollow".to_string(),
                name: "A Hollow".to_string(),
                description: "Mossy.".to_string(),
            },
            &start(),
            &mut seq,
        );
        match resp {
            ApiResponse::Ok { messages } => {
                // The clearing view now lists an east exit.
                assert!(messages.iter().any(|n| matches!(n, PresentationNode::Exits(dirs) if dirs.contains(&Direction::East))));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn unknown_session_is_error() {
        let mut e = engine();
        let mut seq = 0;
        let resp = handle_api_request(&mut e, ApiRequest::Look { session: 999 }, &start(), &mut seq);
        assert!(matches!(resp, ApiResponse::Error { .. }));
    }
}
```

- [ ] **Step 2: Wire into `api/mod.rs`**

Add:
```rust
pub mod handler;

pub use handler::handle_api_request;
```

- [ ] **Step 3: Run tests + fmt + clippy**

Run: `cargo test -p snakewood-daemon api::handler` (4 tests pass), then `cargo fmt -p snakewood-daemon && cargo fmt -p snakewood-daemon -- --check && cargo clippy -p snakewood-daemon --all-targets -- -D warnings`.

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-daemon/src/api/handler.rs crates/snakewood-daemon/src/api/mod.rs
git commit -m "feat: add handle_api_request dispatcher (connect/look/move/dig/disconnect)"
```

---

### Task 6: `serve_api` async server + `main` wiring

**Files:**
- Create: `crates/snakewood-daemon/src/api/server.rs`
- Modify: `crates/snakewood-daemon/src/api/mod.rs`
- Modify: `crates/snakewood-daemon/src/main.rs`

**Interfaces:**
- Consumes: `Rc<RefCell<Engine>>`; `handle_api_request`; `ApiResponse`; tokio net/io; `snakewood_core::EntityId`.
- Produces:
  - `pub async fn serve_api(listener: TcpListener, engine: Rc<RefCell<Engine>>, start_room: EntityId)` — accept loop; each connection `spawn_local`'d; per connection, read JSON lines, dispatch via `handle_api_request` (borrow the engine only in a tight block, never across `.await`), write the JSON response + `\n`. A parse error yields an `ApiResponse::Error` line (connection stays open). A shared per-server `Rc<RefCell<u64>>` mints player seq numbers.
  - `main` binds the API listener on `SNAKEWOOD_API_ADDR` (default `127.0.0.1:4001`) and runs `serve` (telnet) + `serve_api` concurrently via `tokio::join!`, with the tick loop still `spawn_local`'d.

- [ ] **Step 1: Write `crates/snakewood-daemon/src/api/server.rs`**

```rust
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
            Err(err) => ApiResponse::Error { message: format!("bad request: {err}") },
        };
        let mut out = serde_json::to_string(&response).unwrap_or_else(|_| {
            "{\"status\":\"error\",\"message\":\"serialize failed\"}".to_string()
        });
        out.push('\n');
        write_half.write_all(out.as_bytes()).await?;
    }
    Ok(())
}
```

> Confirm: the `engine.borrow_mut()` / `next_player.borrow_mut()` guards live only inside the `match` arm's block and are dropped before `write_half.write_all(...).await`. Do NOT hold either borrow across the `.await`.

- [ ] **Step 2: Wire into `api/mod.rs`**

Add:
```rust
pub mod server;

pub use server::serve_api;
```

- [ ] **Step 3: Wire `main.rs` to run both listeners**

In `crates/snakewood-daemon/src/main.rs`, add the API address env var and run both servers with `tokio::join!`. Add `use snakewood_daemon::api::serve_api;` near the other imports. Change the address setup and the `block_on` body:

```rust
    let addr = std::env::var("SNAKEWOOD_ADDR").unwrap_or_else(|_| "127.0.0.1:4000".to_string());
    let api_addr = std::env::var("SNAKEWOOD_API_ADDR").unwrap_or_else(|_| "127.0.0.1:4001".to_string());
```

and the `local.block_on(&rt, async move { ... })` body becomes:

```rust
    local.block_on(&rt, async move {
        let listener = TcpListener::bind(&addr).await?;
        let api_listener = TcpListener::bind(&api_addr).await?;
        eprintln!("snakewood telnet on {addr}, command API on {api_addr}");
        tokio::task::spawn_local(run_tick_loop(engine.clone(), 1));
        tokio::join!(
            serve(listener, engine.clone(), start_room.clone()),
            serve_api(api_listener, engine, start_room),
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
```

(`serve`/`serve_api` never return, so `join!` runs both forever; the trailing `Ok(())` is unreachable but keeps the type.)

- [ ] **Step 4: Build + fmt + clippy**

Run: `cargo build -p snakewood-daemon` (lib + binary compile), then `cargo fmt -p snakewood-daemon && cargo fmt -p snakewood-daemon -- --check && cargo clippy -p snakewood-daemon --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-daemon/src/api/server.rs crates/snakewood-daemon/src/api/mod.rs crates/snakewood-daemon/src/main.rs
git commit -m "feat: add serve_api JSON socket server and run it alongside telnet"
```

---

### Task 7: End-to-end test — drive the world over the JSON API

**Files:**
- Create: `crates/snakewood-daemon/tests/api_e2e.rs`

**Interfaces:**
- Consumes: public API — `snakewood_daemon::{Engine, ManualClock}`, `snakewood_daemon::api::{serve_api, ApiRequest, ApiResponse}`; `snakewood_core::{Realm, World, Room, Direction, EntityId}`; tokio; `serde_json`.
- Produces: an integration test that binds an ephemeral port, runs `serve_api` on a `LocalSet`, connects a `TcpStream`, and drives `Connect` → `Move` → `Dig` → `Move`(into the dug room) as JSON lines, asserting the structured responses.

- [ ] **Step 1: Write `crates/snakewood-daemon/tests/api_e2e.rs`**

```rust
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use snakewood_core::{Direction, EntityId, PresentationNode, Realm, Room, World};
use snakewood_daemon::api::{serve_api, ApiRequest, ApiResponse};
use snakewood_daemon::{Engine, ManualClock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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
async fn round_trip(
    writer: &mut TcpStream,
    lines: &mut tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
    req: &ApiRequest,
) -> ApiResponse {
    let mut line = serde_json::to_string(req).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
    let resp_line = tokio::time::timeout(Duration::from_millis(500), lines.next_line())
        .await
        .expect("timed out waiting for response")
        .unwrap()
        .expect("connection closed");
    serde_json::from_str(&resp_line).unwrap()
}

#[test]
fn drive_world_over_json_api() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let engine = Rc::new(RefCell::new(two_room_engine()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::task::spawn_local(serve_api(listener, engine, id("snakewood/clearing")));

        // The client uses split halves: a write half and a line-reader.
        let stream = TcpStream::connect(addr).await.unwrap();
        let (read_half, _wh) = stream.into_split();
        let mut reader_lines = BufReader::new(read_half).lines();
        // Reconnect a second stream for writing? No — reuse one stream via a second connect is simpler:
        let mut writer = TcpStream::connect(addr).await.unwrap();
        // NOTE: two separate connections would be two sessions. Instead, use ONE connection:
        drop(reader_lines);
        drop(writer);

        // Single connection, split into reader + writer halves that share the socket.
        let stream = TcpStream::connect(addr).await.unwrap();
        let (read_half, write_half) = stream.into_split();
        let mut lines = BufReader::new(read_half).lines();
        // We need &mut TcpStream for round_trip; instead operate on the write half directly here.
        let mut write_half = write_half;

        // Connect.
        {
            let mut line = serde_json::to_string(&ApiRequest::Connect).unwrap();
            line.push('\n');
            write_half.write_all(line.as_bytes()).await.unwrap();
        }
        let connected: ApiResponse = {
            let l = tokio::time::timeout(Duration::from_millis(500), lines.next_line())
                .await.unwrap().unwrap().unwrap();
            serde_json::from_str(&l).unwrap()
        };
        let session = match connected {
            ApiResponse::Connected { session, ref view, .. } => {
                assert!(view.contains(&PresentationNode::RoomName("Snakewood Clearing".to_string())));
                session
            }
            other => panic!("expected Connected, got {other:?}"),
        };

        // Helper closure to send a request and read a response on this single connection.
        async fn exchange(
            write_half: &mut tokio::net::tcp::OwnedWriteHalf,
            lines: &mut tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
            req: &ApiRequest,
        ) -> ApiResponse {
            let mut line = serde_json::to_string(req).unwrap();
            line.push('\n');
            write_half.write_all(line.as_bytes()).await.unwrap();
            let l = tokio::time::timeout(Duration::from_millis(500), lines.next_line())
                .await.unwrap().unwrap().unwrap();
            serde_json::from_str(&l).unwrap()
        }

        // Dig east into a new hollow; the returned view lists an east exit.
        let dug = exchange(&mut write_half, &mut lines, &ApiRequest::Dig {
            session,
            direction: Direction::East,
            id: "snakewood/hollow".to_string(),
            name: "A Hollow".to_string(),
            description: "A mossy hollow.".to_string(),
        }).await;
        match dug {
            ApiResponse::Ok { messages } => assert!(messages.iter().any(|n|
                matches!(n, PresentationNode::Exits(dirs) if dirs.contains(&Direction::East)))),
            other => panic!("expected Ok from dig, got {other:?}"),
        }

        // Move east into the room we just dug.
        let moved = exchange(&mut write_half, &mut lines, &ApiRequest::Move {
            session, direction: Direction::East,
        }).await;
        match moved {
            ApiResponse::Ok { messages } => assert!(messages.contains(
                &PresentationNode::RoomName("A Hollow".to_string()))),
            other => panic!("expected Ok from move, got {other:?}"),
        }
    });
}
```

> NOTE TO IMPLEMENTER: the block above contains an exploratory false start (the first `reader_lines`/`writer` pair that is immediately `drop`ped). DELETE that dead false-start; keep only the single-connection flow (`let stream = TcpStream::connect(addr)...; let (read_half, write_half) = stream.into_split();` onward) plus the `exchange` helper. Also delete the unused top-level `round_trip` fn if you keep `exchange` (or vice-versa) — ship exactly ONE request/response helper and no dead code, so `clippy -D warnings` passes. The test must: Connect (assert start-room view), Dig east (assert east exit appears), Move east (assert "A Hollow"). Keep the 500ms timeouts.

- [ ] **Step 2: Run the test + fmt + clippy**

Run: `cargo test -p snakewood-daemon --test api_e2e` (passes), then `cargo fmt -p snakewood-daemon && cargo fmt -p snakewood-daemon -- --check && cargo clippy -p snakewood-daemon --all-targets -- -D warnings` (clean — no dead code).

- [ ] **Step 3: Commit**

```bash
git add crates/snakewood-daemon/tests/api_e2e.rs
git commit -m "test: drive connect/move/dig over the JSON command API end-to-end"
```

---

### Task 8: Stage-completion verification

**Files:** none (verification only).

- [ ] **Step 1: Full workspace suite**

Run: `cargo test --workspace`
Expected: all pass — `snakewood-core` (incl. new presentation/direction tests) and `snakewood-daemon` (engine incl. `dig`, api protocol/handler, telnet, integration tests incl. `api_e2e` + `telnet_e2e`).

- [ ] **Step 2: Clippy + fmt**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no diffs. If diffs, `cargo fmt`, re-run, commit.

- [ ] **Step 3: Manual smoke — build the world over the API**

```bash
DATA="$(mktemp -d)/world"
SNAKEWOOD_DATA="$DATA" SNAKEWOOD_ADDR=127.0.0.1:4088 SNAKEWOOD_API_ADDR=127.0.0.1:4089 cargo run -p snakewood-daemon >/tmp/sw_api.log 2>&1 &
SVPID=$!
sleep 4
printf '{"op":"connect"}\n{"op":"dig","session":0,"direction":"East","id":"snakewood/hollow","name":"A Hollow","description":"Mossy."}\n{"op":"move","session":0,"direction":"East"}\n' | nc -w2 127.0.0.1 4089 > /tmp/sw_api_client.out || true
kill "$SVPID" 2>/dev/null || true; wait "$SVPID" 2>/dev/null || true
echo "=== API client output ==="; cat /tmp/sw_api_client.out
```
Expected: three JSON response lines — a `connected` with the clearing view, an `ok` whose view lists an `East` exit, and an `ok` with `"A Hollow"`. (Skip if `nc` unavailable — `api_e2e` already proves it.)

- [ ] **Step 4: Commit any fmt fixes**

```bash
git add -A
git commit -m "chore: stage 3d verification — clippy clean, cargo fmt, workspace green"
```
(Skip if nothing to commit.)

---

## Self-Review

**1. Spec coverage (spec §2 structured API for the MCP; §7 M1 OOC `dig`):**
- Structured command channel (JSON), distinct from telnet's text — Tasks 4, 6. ✓
- IC `look`/`move` return structured `PresentationNode`s — Tasks 1, 5. ✓
- OOC `dig` (create+link room) as the first authored op, calling `checkpoint` (on-change policy) — Tasks 2, 3, 5. ✓
- Persisted authored change survives a boot — Task 3 test. ✓
- Same session/actor authorization as telnet (`Engine::submit`) — Task 5 (unknown session → Error). ✓
- Correctly deferred: the MCP-protocol bridge binary (Stage 3d-mcp), auth/accounts, room deletion. ✓

**2. Placeholder scan:** No "TBD/TODO". Task 7's e2e contains a clearly-labeled exploratory false-start with an explicit DELETE instruction and a single-helper directive (a guardrail so the shipped test is clean/clippy-passing), not an unfinished placeholder. The manual smoke (Task 8 Step 3) is explicitly optional.

**3. Type consistency:** `ApiRequest`/`ApiResponse` variants and their fields (`session: u64`, `direction: Direction`, `view`/`messages: Vec<PresentationNode>`) are used identically across Tasks 4–7. `handle_api_request(&mut Engine, ApiRequest, &EntityId, &mut u64) -> ApiResponse` consistent (Tasks 5–6). `Engine::dig(SessionId, Direction, &str, &str, &str) -> Result<EntityId, DigError>` consistent (Tasks 3, 5). `Direction::opposite()` (Task 2) used by `dig` (Task 3). `PresentationNode` serde (Task 1) is required by `ApiResponse` (Task 4) and the wire. `serve_api(TcpListener, Rc<RefCell<Engine>>, EntityId)` matches `main`'s `join!` call (Task 6). `SessionId(pub u64)` ↔ the wire's `u64` conversions are explicit at every boundary. Borrows in `serve_api` are dropped before `.await`. ✓
