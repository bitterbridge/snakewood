# Snakewood M1 Stage 3a — Daemon Engine (sync core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the synchronous heart of the long-lived daemon — an `Engine` that owns the `Realm`, holds an injected `Clock`, tracks connected sessions, routes dispatched presentation to the right session's outbox, and advances a tick counter — so later sub-stages can wrap it in async I/O (telnet, MCP) without the world logic living in the network layer.

**Architecture:** A new `snakewood-daemon` crate depends on `snakewood-core`. The `Engine` is pure and synchronous: `submit(session, intent)` calls `snakewood_core::dispatch` against the owned `Realm` and fans the resulting `PresentationNode`s out to the session(s) whose actor matches each message's recipient; `poll(session)` drains a session's buffer. Time is an injected `Clock` trait (a lock-free `ManualClock` for tests) so the tick loop and later persistence intervals are deterministic. This sub-stage also finishes a Stage-2 carry-forward: the "no exit" fallback message becomes data on `Realm` instead of a hardcoded string.

**Tech Stack:** Rust (edition 2021), existing `snakewood-core`, `std` only (no async, no new external deps in 3a). `std::sync::atomic::AtomicI64` for the test clock.

## Global Constraints

- **Language:** Rust, edition 2021. New crate `snakewood-daemon` is a workspace member depending on `snakewood-core` by path. No new EXTERNAL dependencies in this sub-stage (async/tokio arrives in 3c).
- **Pure & synchronous:** no async, no threads spawned, no real I/O, no wall-clock reads. The `Engine` is a deterministic function of its inputs + injected `Clock`.
- **Injected time:** all time comes through the `Clock` trait; tests use `ManualClock`. Never call `SystemTime::now()`/`Instant::now()` in library code.
- **No embedded scripting.** Behavior stays data-driven via the fabric.
- **`World` FROZEN**; `Realm` may gain fields but Stage 1 round-trip/golden tests must stay green. `snakewood-core`'s existing public API must not break (only additive changes).
- **Collections:** `BTreeMap`/`BTreeSet`, never `HashMap`/`HashSet`, so ordering is deterministic.
- **`Clock` is `Send + Sync`** (forward-compat for the async daemon in 3c); `ManualClock` uses `AtomicI64` to satisfy this.
- **Deferred to later sub-stages (do NOT build):** async runtime / tokio, TCP/telnet, MCP, persistence scheduler & the `state/` lane, real system clock, tick-triggered behavior (there are no `Tick` responders yet).

---

### Task 1: "No exit" fallback becomes data on `Realm`

**Files:**
- Modify: `crates/snakewood-core/src/realm.rs`
- Modify: `crates/snakewood-core/src/fabric/dispatch.rs`

**Interfaces:**
- Consumes: existing `Realm`, `dispatch`.
- Produces:
  - `Realm` gains `pub no_exit_message: String`. `Realm::new(world)` initializes it to `"You see no exit in that direction."`. `Realm` no longer derives `Default`; a manual `impl Default for Realm` delegates to `Realm::new(World::default())` so existing `Realm::default()` callers keep the sensible message.
  - `dispatch`'s `Decision::Unresolved` arm emits `PresentationNode::Denied(realm.no_exit_message.clone())` instead of a hardcoded literal.

- [ ] **Step 1: Update `Realm` — add the field, replace derived Default with a manual impl**

In `crates/snakewood-core/src/realm.rs`, change the struct's derive line from `#[derive(Debug, Clone, Default)]` to `#[derive(Debug, Clone)]`, add the field, initialize it in `new`, and add a manual `Default`. The struct and impl become:

```rust
#[derive(Debug, Clone)]
pub struct Realm {
    pub world: World,
    pub mobs: BTreeMap<EntityId, Mob>,
    pub rules: Vec<Rule>,
    /// Message shown when a movement intent resolves to no exit at all.
    /// Data, not hardcoded, so content can reword/localize it.
    pub no_exit_message: String,
}

impl Realm {
    pub fn new(world: World) -> Realm {
        Realm {
            world,
            mobs: BTreeMap::new(),
            rules: Vec::new(),
            no_exit_message: "You see no exit in that direction.".to_string(),
        }
    }
    // ... existing methods (insert_mob, mob, mob_mut, mob_location, mobs_in_room) unchanged ...
}

impl Default for Realm {
    fn default() -> Realm {
        Realm::new(World::default())
    }
}
```

Keep every existing method body in the `impl Realm` block exactly as-is; only the struct definition, `new`, and the new `Default` impl change.

- [ ] **Step 2: Update `dispatch` to use the data-driven message**

In `crates/snakewood-core/src/fabric/dispatch.rs`, the `Decision::Unresolved` arm currently pushes a hardcoded string. Replace it:

```rust
                Decision::Unresolved => {
                    out.messages.push((
                        actor.clone(),
                        PresentationNode::Denied(realm.no_exit_message.clone()),
                    ));
                }
```

- [ ] **Step 3: Add a test in `realm.rs` for the default message**

In the `#[cfg(test)] mod tests` of `crates/snakewood-core/src/realm.rs`, add:

```rust
    #[test]
    fn realm_has_default_no_exit_message() {
        let realm = Realm::new(World::default());
        assert_eq!(realm.no_exit_message, "You see no exit in that direction.");
        // Default delegates to new(World::default())
        assert_eq!(Realm::default().no_exit_message, realm.no_exit_message);
    }
```

- [ ] **Step 4: Update the dispatch no-exit test to prove it reads the field**

In `crates/snakewood-core/src/fabric/dispatch.rs` tests, the existing `move_with_no_exit_is_denied_with_fallback_message` asserts the literal string. Keep it, and add a test proving the message is data-driven (edit the field, see it reflected). Add after that test:

```rust
    #[test]
    fn no_exit_message_is_data_driven() {
        let mut realm = realm_with_actor();
        realm.no_exit_message = "There's nothing that way, friend.".to_string();
        let out = dispatch(
            &mut realm,
            Intent::Move { actor: actor_id(), direction: Direction::South },
        );
        assert!(out.messages.iter().any(|(_, n)| *n
            == PresentationNode::Denied("There's nothing that way, friend.".to_string())));
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p snakewood-core`
Expected: all pass, including `realm_has_default_no_exit_message`, `no_exit_message_is_data_driven`, the existing `move_with_no_exit_is_denied_with_fallback_message`, and all Stage 1/2 tests (World frozen; round-trip + golden green).

- [ ] **Step 6: Commit**

```bash
git add crates/snakewood-core/src/realm.rs crates/snakewood-core/src/fabric/dispatch.rs
git commit -m "feat: make the no-exit fallback message data on Realm"
```

---

### Task 2: `snakewood-daemon` crate scaffold + `Clock`

**Files:**
- Modify: `Cargo.toml` (workspace members)
- Create: `crates/snakewood-daemon/Cargo.toml`
- Create: `crates/snakewood-daemon/src/lib.rs`
- Create: `crates/snakewood-daemon/src/clock.rs`

**Interfaces:**
- Consumes: nothing from the daemon yet (first daemon file); depends on `snakewood-core` for later tasks.
- Produces:
  - A workspace member crate `snakewood-daemon` depending on `snakewood-core` (path).
  - `pub trait Clock: Send + Sync { fn now_unix(&self) -> i64; }`.
  - `pub struct ManualClock` (uses `AtomicI64`) with `ManualClock::new(start: i64) -> ManualClock`, `set(&self, t: i64)`, `advance(&self, secs: i64)`, and `impl Clock`.

- [ ] **Step 1: Add the crate to the workspace `Cargo.toml`**

Change the `members` array in the root `Cargo.toml` to include the daemon:

```toml
members = ["crates/snakewood-core", "crates/snakewood-daemon"]
```

- [ ] **Step 2: Write `crates/snakewood-daemon/Cargo.toml`**

```toml
[package]
name = "snakewood-daemon"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
snakewood-core = { path = "../snakewood-core" }
```

- [ ] **Step 3: Write `crates/snakewood-daemon/src/clock.rs` with tests**

```rust
use std::sync::atomic::{AtomicI64, Ordering};

/// Injected time source. `Send + Sync` so the async daemon (Stage 3c) can share it.
pub trait Clock: Send + Sync {
    /// Current time in whole Unix seconds.
    fn now_unix(&self) -> i64;
}

/// A test/manual clock whose time only changes when explicitly told.
pub struct ManualClock {
    now: AtomicI64,
}

impl ManualClock {
    pub fn new(start: i64) -> ManualClock {
        ManualClock { now: AtomicI64::new(start) }
    }

    pub fn set(&self, t: i64) {
        self.now.store(t, Ordering::Relaxed);
    }

    pub fn advance(&self, secs: i64) {
        self.now.fetch_add(secs, Ordering::Relaxed);
    }
}

impl Clock for ManualClock {
    fn now_unix(&self) -> i64 {
        self.now.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_starts_at_and_advances() {
        let clock = ManualClock::new(1000);
        assert_eq!(clock.now_unix(), 1000);
        clock.advance(60);
        assert_eq!(clock.now_unix(), 1060);
        clock.set(5);
        assert_eq!(clock.now_unix(), 5);
    }
}
```

- [ ] **Step 4: Write `crates/snakewood-daemon/src/lib.rs`**

```rust
//! snakewood-daemon: the long-lived host around the pure `snakewood-core` engine.
//! Stage 3a is synchronous; async transports wrap this in later sub-stages.

pub mod clock;

pub use clock::{Clock, ManualClock};
```

- [ ] **Step 5: Build and test**

Run: `cargo test -p snakewood-daemon`
Expected: compiles; `clock::tests::manual_clock_starts_at_and_advances` passes.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/snakewood-daemon/Cargo.toml crates/snakewood-daemon/src/lib.rs crates/snakewood-daemon/src/clock.rs
git commit -m "feat: scaffold snakewood-daemon crate with injected Clock"
```

---

### Task 3: Sessions and the `Engine` skeleton

**Files:**
- Create: `crates/snakewood-daemon/src/session.rs`
- Create: `crates/snakewood-daemon/src/engine.rs`
- Modify: `crates/snakewood-daemon/src/lib.rs`

**Interfaces:**
- Consumes: `snakewood_core::{Realm, EntityId, PresentationNode}`; `crate::Clock`.
- Produces:
  - `pub struct SessionId(pub u64)` deriving `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`.
  - `pub struct Session { pub actor: EntityId, pub outbox: Vec<PresentationNode> }` deriving `Debug, Clone`.
  - `pub struct Engine { realm, clock, sessions, next_session, tick }` (fields private).
  - `Engine::new(realm: Realm, clock: Box<dyn Clock>) -> Engine`.
  - `Engine::connect(&mut self, actor: EntityId) -> SessionId` (assigns a fresh id, empty outbox).
  - `Engine::disconnect(&mut self, id: SessionId) -> Option<Session>`.
  - `Engine::session_actor(&self, id: SessionId) -> Option<&EntityId>`.
  - `Engine::realm(&self) -> &Realm` and `Engine::realm_mut(&mut self) -> &mut Realm` (accessors, needed by later tasks/tests).

- [ ] **Step 1: Write `crates/snakewood-daemon/src/session.rs`**

```rust
use snakewood_core::{EntityId, PresentationNode};

/// Identifies a connected session (one per client connection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

/// A connected session: the actor it drives and its pending outbound view.
#[derive(Debug, Clone)]
pub struct Session {
    pub actor: EntityId,
    pub outbox: Vec<PresentationNode>,
}

impl Session {
    pub fn new(actor: EntityId) -> Session {
        Session { actor, outbox: Vec::new() }
    }
}
```

- [ ] **Step 2: Write `crates/snakewood-daemon/src/engine.rs` skeleton with tests**

```rust
use std::collections::BTreeMap;

use snakewood_core::{EntityId, Realm};

use crate::session::{Session, SessionId};
use crate::Clock;

/// The synchronous core of the daemon: owns the world, the clock, and sessions.
pub struct Engine {
    realm: Realm,
    clock: Box<dyn Clock>,
    sessions: BTreeMap<SessionId, Session>,
    next_session: u64,
    tick: u64,
}

impl Engine {
    pub fn new(realm: Realm, clock: Box<dyn Clock>) -> Engine {
        Engine {
            realm,
            clock,
            sessions: BTreeMap::new(),
            next_session: 0,
            tick: 0,
        }
    }

    /// Register a new session bound to `actor`; returns its id.
    pub fn connect(&mut self, actor: EntityId) -> SessionId {
        let id = SessionId(self.next_session);
        self.next_session += 1;
        self.sessions.insert(id, Session::new(actor));
        id
    }

    /// Remove a session, returning it if present.
    pub fn disconnect(&mut self, id: SessionId) -> Option<Session> {
        self.sessions.remove(&id)
    }

    pub fn session_actor(&self, id: SessionId) -> Option<&EntityId> {
        self.sessions.get(&id).map(|s| &s.actor)
    }

    pub fn realm(&self) -> &Realm {
        &self.realm
    }

    pub fn realm_mut(&mut self) -> &mut Realm {
        &mut self.realm
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualClock;
    use snakewood_core::World;

    fn engine() -> Engine {
        Engine::new(Realm::new(World::default()), Box::new(ManualClock::new(0)))
    }

    #[test]
    fn connect_assigns_distinct_ids_and_binds_actor() {
        let mut e = engine();
        let a = EntityId::new("snakewood/pc/a").unwrap();
        let b = EntityId::new("snakewood/pc/b").unwrap();
        let sa = e.connect(a.clone());
        let sb = e.connect(b.clone());
        assert_ne!(sa, sb);
        assert_eq!(e.session_actor(sa), Some(&a));
        assert_eq!(e.session_actor(sb), Some(&b));
    }

    #[test]
    fn disconnect_removes_session() {
        let mut e = engine();
        let a = EntityId::new("snakewood/pc/a").unwrap();
        let sa = e.connect(a);
        assert!(e.disconnect(sa).is_some());
        assert_eq!(e.session_actor(sa), None);
        assert!(e.disconnect(sa).is_none());
    }
}
```

- [ ] **Step 3: Wire modules into `lib.rs`**

Update `crates/snakewood-daemon/src/lib.rs`:

```rust
//! snakewood-daemon: the long-lived host around the pure `snakewood-core` engine.
//! Stage 3a is synchronous; async transports wrap this in later sub-stages.

pub mod clock;
pub mod engine;
pub mod session;

pub use clock::{Clock, ManualClock};
pub use engine::Engine;
pub use session::{Session, SessionId};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p snakewood-daemon engine`
Expected: PASS (`connect_assigns_distinct_ids_and_binds_actor`, `disconnect_removes_session`).

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-daemon/src/session.rs crates/snakewood-daemon/src/engine.rs crates/snakewood-daemon/src/lib.rs
git commit -m "feat: add Engine skeleton with session registry"
```

---

### Task 4: `submit` — dispatch an intent and route presentation to sessions

**Files:**
- Modify: `crates/snakewood-daemon/src/engine.rs`

**Interfaces:**
- Consumes: `snakewood_core::{dispatch, Intent, PresentationNode}`; the `Engine` from Task 3.
- Produces:
  - `Engine::submit(&mut self, id: SessionId, intent: Intent)` — dispatches `intent` against the owned `Realm` via `snakewood_core::dispatch`, then routes each resulting `(recipient, node)` message to the outbox of every session whose `actor == recipient`. A no-op if `id` is not a live session.
  - `Engine::poll(&mut self, id: SessionId) -> Vec<PresentationNode>` — drains and returns a session's outbox (empty if none/unknown).

- [ ] **Step 1: Write the failing tests (add to `engine.rs` tests module)**

Add these tests inside the existing `#[cfg(test)] mod tests` in `crates/snakewood-daemon/src/engine.rs`. They need a small two-room world + an actor; add the helpers and tests:

```rust
    use snakewood_core::{Direction, Flag, Intent, Mob, PresentationNode, Room};
    use std::collections::{BTreeMap, BTreeSet};

    fn world_two_rooms() -> World {
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
        world
    }

    fn engine_with_actor() -> (Engine, SessionId, EntityId) {
        let mut realm = Realm::new(world_two_rooms());
        let actor = EntityId::new("snakewood/pc/nathan").unwrap();
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        realm.insert_mob(Mob {
            id: actor.clone(),
            name: "Nathan".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        });
        let mut e = Engine::new(realm, Box::new(ManualClock::new(0)));
        let sid = e.connect(actor.clone());
        (e, sid, actor)
    }

    #[test]
    fn submit_move_routes_arrival_view_to_session_and_relocates() {
        let (mut e, sid, actor) = engine_with_actor();
        e.submit(sid, Intent::Move { actor: actor.clone(), direction: Direction::North });
        // world state changed
        assert_eq!(e.realm().mob_location(&actor).map(|r| r.as_str()), Some("snakewood/old-well"));
        // arrival view delivered to the session
        let view = e.poll(sid);
        assert!(view.contains(&PresentationNode::RoomName("The Old Well".to_string())));
        // draining leaves the outbox empty
        assert!(e.poll(sid).is_empty());
    }

    #[test]
    fn submit_move_no_exit_routes_fallback_message() {
        let (mut e, sid, actor) = engine_with_actor();
        e.submit(sid, Intent::Move { actor, direction: Direction::South });
        let view = e.poll(sid);
        assert!(view.contains(&PresentationNode::Denied("You see no exit in that direction.".to_string())));
    }

    #[test]
    fn submit_on_unknown_session_is_noop() {
        let (mut e, _sid, actor) = engine_with_actor();
        e.submit(SessionId(999), Intent::Look { actor });
        assert!(e.poll(SessionId(999)).is_empty());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p snakewood-daemon engine`
Expected: FAIL to compile — `submit`/`poll` don't exist yet.

- [ ] **Step 3: Implement `submit` and `poll`**

Add to the `impl Engine` block in `crates/snakewood-daemon/src/engine.rs` (and add the import at the top of the file: `use snakewood_core::{dispatch, EntityId, Intent, PresentationNode, Realm};` — merge with the existing `use snakewood_core::{EntityId, Realm};` line):

```rust
    /// Dispatch `intent` and fan the resulting presentation out to sessions.
    pub fn submit(&mut self, id: SessionId, intent: Intent) {
        if !self.sessions.contains_key(&id) {
            return;
        }
        let result = dispatch(&mut self.realm, intent);
        for (recipient, node) in result.messages {
            for session in self.sessions.values_mut() {
                if session.actor == recipient {
                    session.outbox.push(node.clone());
                }
            }
        }
    }

    /// Drain a session's pending presentation.
    pub fn poll(&mut self, id: SessionId) -> Vec<PresentationNode> {
        match self.sessions.get_mut(&id) {
            Some(session) => std::mem::take(&mut session.outbox),
            None => Vec::new(),
        }
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p snakewood-daemon engine`
Expected: PASS (the three new `submit_*` tests plus the Task 3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-daemon/src/engine.rs
git commit -m "feat: Engine.submit dispatches and routes presentation to sessions"
```

---

### Task 5: The tick loop hook

**Files:**
- Modify: `crates/snakewood-daemon/src/engine.rs`

**Interfaces:**
- Consumes: the `Engine`.
- Produces:
  - `Engine::tick(&mut self) -> u64` — advances the logical tick counter and returns the new count. (No tick-triggered behavior exists yet; this establishes the loop hook that Stage 3b's persistence scheduler and later `Tick` responders will use.)
  - `Engine::tick_count(&self) -> u64`.
  - `Engine::now_unix(&self) -> i64` — the current injected time (delegates to the clock).

- [ ] **Step 1: Write the failing tests (add to `engine.rs` tests)**

```rust
    #[test]
    fn tick_advances_counter() {
        let mut e = engine();
        assert_eq!(e.tick_count(), 0);
        assert_eq!(e.tick(), 1);
        assert_eq!(e.tick(), 2);
        assert_eq!(e.tick_count(), 2);
    }

    #[test]
    fn now_unix_reflects_injected_clock() {
        let clock = ManualClock::new(500);
        // Keep a raw pointer-free handle by advancing before moving into the engine.
        clock.advance(100); // now 600
        let e = Engine::new(Realm::new(World::default()), Box::new(clock));
        assert_eq!(e.now_unix(), 600);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p snakewood-daemon engine::tests::tick_advances_counter`
Expected: FAIL to compile — `tick`/`tick_count`/`now_unix` don't exist.

- [ ] **Step 3: Implement**

Add to the `impl Engine` block:

```rust
    /// Advance the logical tick counter by one; returns the new count.
    pub fn tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    /// Current injected time in Unix seconds.
    pub fn now_unix(&self) -> i64 {
        self.clock.now_unix()
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p snakewood-daemon engine`
Expected: PASS (all engine tests, including the two new ones).

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-daemon/src/engine.rs
git commit -m "feat: add Engine tick loop hook and injected time accessor"
```

---

### Task 6: Integration test — the blocking goblin through the Engine

**Files:**
- Create: `crates/snakewood-daemon/tests/engine_goblin.rs`

**Interfaces:**
- Consumes: the daemon's public API — `snakewood_daemon::{Engine, ManualClock, SessionId}` and `snakewood_core::{Realm, World, Room, Mob, Flag, EntityId, Direction, Intent, PresentationNode}` plus `snakewood_core::fabric::{Responder, Trigger, Predicate, Party, Effect, Outcome}`.
- Produces: an integration test proving the daemon `Engine` drives the fabric end-to-end — a session's `Move` is blocked by a conscious goblin (block message delivered to that session), and after the goblin is knocked unconscious via a pure state change on the engine's realm, the same `Move` succeeds and the arrival view is delivered.

- [ ] **Step 1: Write `crates/snakewood-daemon/tests/engine_goblin.rs`**

```rust
use std::collections::{BTreeMap, BTreeSet};

use snakewood_core::fabric::{Effect, Outcome, Party, Predicate, Responder, Trigger};
use snakewood_core::{
    Direction, EntityId, Flag, Intent, Mob, PresentationNode, Realm, Room, World,
};
use snakewood_daemon::{Engine, ManualClock};

fn id(s: &str) -> EntityId {
    EntityId::new(s).unwrap()
}

fn world() -> World {
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

fn actor() -> Mob {
    let mut flags = BTreeSet::new();
    flags.insert(Flag::Alive);
    Mob {
        id: id("snakewood/pc/nathan"),
        name: "Nathan".to_string(),
        location: id("snakewood/clearing"),
        flags,
        responders: Vec::new(),
    }
}

fn goblin() -> Mob {
    let mut flags = BTreeSet::new();
    flags.insert(Flag::Alive);
    flags.insert(Flag::Conscious);
    Mob {
        id: id("snakewood/mob/goblin#1"),
        name: "a snakewood goblin".to_string(),
        location: id("snakewood/clearing"),
        flags,
        responders: vec![Responder {
            on: Trigger::Move(Direction::North),
            require: vec![Predicate::Alive(Party::SelfMob), Predicate::Conscious(Party::SelfMob)],
            effects: vec![Effect::Narrate(Party::Actor, "The goblin blocks your way north.".to_string())],
            outcome: Outcome::Block,
            priority: 0,
        }],
    }
}

fn engine_with_scene() -> Engine {
    let mut realm = Realm::new(world());
    realm.insert_mob(actor());
    realm.insert_mob(goblin());
    Engine::new(realm, Box::new(ManualClock::new(0)))
}

#[test]
fn engine_delivers_block_then_passage_after_incapacitation() {
    let mut e = engine_with_scene();
    let sid = e.connect(id("snakewood/pc/nathan"));

    // Conscious goblin blocks: the session receives the block line, no relocation.
    e.submit(sid, Intent::Move { actor: id("snakewood/pc/nathan"), direction: Direction::North });
    assert_eq!(e.realm().mob_location(&id("snakewood/pc/nathan")).map(|r| r.as_str()), Some("snakewood/clearing"));
    let view = e.poll(sid);
    assert!(view.contains(&PresentationNode::Line("The goblin blocks your way north.".to_string())));

    // Knock the goblin unconscious — pure state change on the realm, no wiring edits.
    e.realm_mut()
        .mob_mut(&id("snakewood/mob/goblin#1"))
        .unwrap()
        .flags
        .remove(&Flag::Conscious);

    // Same intent now passes; arrival view delivered.
    e.submit(sid, Intent::Move { actor: id("snakewood/pc/nathan"), direction: Direction::North });
    assert_eq!(e.realm().mob_location(&id("snakewood/pc/nathan")).map(|r| r.as_str()), Some("snakewood/old-well"));
    let view = e.poll(sid);
    assert!(view.contains(&PresentationNode::RoomName("The Old Well".to_string())));
    assert!(!view.contains(&PresentationNode::Line("The goblin blocks your way north.".to_string())));
}
```

- [ ] **Step 2: Run the integration test**

Run: `cargo test -p snakewood-daemon --test engine_goblin`
Expected: PASS (`engine_delivers_block_then_passage_after_incapacitation`).

- [ ] **Step 3: Commit**

```bash
git add crates/snakewood-daemon/tests/engine_goblin.rs
git commit -m "test: prove the Engine drives the blocking-goblin scenario end-to-end"
```

---

### Task 7: Stage-completion verification

**Files:** none (verification only).

- [ ] **Step 1: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: all pass — `snakewood-core` (Stage 1 + 2, unchanged behavior; round-trip + golden green) and `snakewood-daemon` (clock, engine, engine_goblin).

- [ ] **Step 2: Clippy and formatting across the workspace**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no diffs. If `cargo fmt --check` reports diffs, run `cargo fmt`, re-run the suite, and commit.

- [ ] **Step 3: Commit any formatting fixes**

```bash
git add -A
git commit -m "chore: stage 3a verification — clippy clean, cargo fmt, workspace green"
```
(Skip if there was nothing to commit.)

---

## Self-Review

**1. Spec coverage (spec §2 daemon host + §7 M1 "daemon" slice; Stage 3a portion):**
- Long-lived daemon core owning the world — `Engine` (Tasks 3–5). ✓
- Injected `Clock` from the start — Task 2 (`Clock`/`ManualClock`), used by `Engine` (Task 5). ✓
- Session registry + routing presentation to the right client — Tasks 3–4. ✓
- Tick loop hook — Task 5. ✓
- Carry-forward: no-exit fallback as data — Task 1. ✓
- Correctly deferred to 3b–3d (absent here): persistence scheduler/`state/` lane, telnet, MCP, async/tokio, real clock. ✓

**2. Placeholder scan:** No "TBD/TODO". Every step has complete code. The tick loop's "no behavior yet" is an explicit, documented hook (there are no `Tick` responders in the fabric yet), not a placeholder. ✓

**3. Type consistency:** `Engine::new(Realm, Box<dyn Clock>)`, `connect(EntityId) -> SessionId`, `submit(SessionId, Intent)`, `poll(SessionId) -> Vec<PresentationNode>`, `tick() -> u64`, `realm()/realm_mut()` used consistently across Tasks 3–6. `Clock::now_unix(&self) -> i64` and `ManualClock::{new,set,advance}` consistent (Tasks 2, 5). `SessionId(pub u64)`, `Session { actor, outbox }` consistent. `Realm.no_exit_message` added in Task 1 and relied on by dispatch; `Realm` loses derived `Default` in favor of a manual impl (Task 1) — existing `Realm::default()`/`Realm::new` callers keep working. All `Mob { .. }` literals in daemon tests include the `responders` field (present since Stage 2). ✓
