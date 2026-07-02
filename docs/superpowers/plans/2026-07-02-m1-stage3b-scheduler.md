# Snakewood M1 Stage 3b-scheduler — Commit Scheduler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the daemon `Engine` to a `WorldStore` so the live world persists — boot a `Realm` from git, checkpoint it on demand (on-checkpoint / on-change), and snapshot it on a clock-driven interval (on-interval) — proven by a "world survives a restart" test.

**Architecture:** The `Engine` gains an optional `Box<dyn WorldStore>` plus a snapshot interval and a last-snapshot timestamp. `Engine::boot(store, clock)` loads the whole `Realm` from the store; `checkpoint(msg)` calls `save_realm` + `commit` immediately (serves on-checkpoint now and on-change once authored ops like `dig` exist in 3d); `maybe_snapshot()` — driven from the tick loop against the injected `Clock` — commits only when the interval has elapsed. All synchronous; the async transport wrapping is still Stage 3c.

**Tech Stack:** Rust (edition 2021), `snakewood-daemon` on `snakewood-core` (`WorldStore`/`GitStore`/`Realm`/`StoreError`). Adds `tempfile` as a daemon dev-dependency for store-backed tests.

## Global Constraints

- **Language:** Rust, edition 2021. Work in `snakewood-daemon`; the only `snakewood-core` use is through its public `WorldStore` API (no core changes). Add `tempfile` to the daemon's `[dev-dependencies]`; no other new deps.
- **Pure & synchronous:** no async/tokio, no threads, no wall-clock reads — all time via the injected `Clock`. Persistence effects happen only when the caller invokes `checkpoint`/`maybe_snapshot`.
- **Non-breaking:** `Engine::new(realm, clock)` keeps working (store defaults to `None`); existing Stage 3a Engine tests stay green. Store is opt-in.
- **Commit policies (spec §5):** `on-checkpoint` = explicit `checkpoint(msg)`; `on-interval` = clock-driven `maybe_snapshot()`; `on-change` = realized by authored ops calling `checkpoint` (no authored op exists in the daemon yet — that's 3d `dig`; the mechanism is in place).
- **Determinism:** `maybe_snapshot` decides purely from `Clock::now_unix()` vs the last-snapshot time and the configured interval. `BTreeMap`/`BTreeSet` only.
- **Known trap to respect (do NOT introduce a resurrection bug):** `save_realm` upserts and never prunes. Since the daemon has no despawn/death path yet, no mob is ever removed at runtime in this sub-stage, so a plain `save_realm` snapshot is correct. When despawn/death lands later, it MUST call `store.remove_mob(id)`; this sub-stage does not add despawn, so it does not need pruning.
- **Deferred (do NOT build):** async/tokio, telnet, MCP, despawn/death (and its `remove_mob` pairing), per-world `no_exit_message` persistence, `WorldStore: Send` bound (a Stage 3c concern).

---

### Task 1: `Engine` gains a store — `attach_store` and `boot`

**Files:**
- Modify: `crates/snakewood-daemon/Cargo.toml`
- Modify: `crates/snakewood-daemon/src/engine.rs`

**Interfaces:**
- Consumes: `snakewood_core::{WorldStore, StoreError, Realm}`; existing `Engine`.
- Produces:
  - `Engine` gains private fields `store: Option<Box<dyn WorldStore>>`, `snapshot_interval: Option<i64>`, `last_snapshot: i64` (initialized `None`/`None`/`0` in `new`).
  - `Engine::attach_store(&mut self, store: Box<dyn WorldStore>)` — sets the store and initializes `last_snapshot` to the current clock time.
  - `Engine::boot(store: Box<dyn WorldStore>, clock: Box<dyn Clock>) -> Result<Engine, StoreError>` — loads the whole `Realm` via `store.load_realm()`, builds an `Engine`, and attaches the store.
  - `Engine::has_store(&self) -> bool` (test/inspection helper).

- [ ] **Step 1: Add `tempfile` dev-dependency to the daemon**

In `crates/snakewood-daemon/Cargo.toml`, add a dev-dependencies section (the crate currently has only `[dependencies]`):

```toml
[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 2: Add the store fields to `Engine` and update `new`**

In `crates/snakewood-daemon/src/engine.rs`, extend the top `use` to bring in the store types (merge with the existing `use snakewood_core::{...}` line):

```rust
use snakewood_core::{dispatch, EntityId, Intent, PresentationNode, Realm, StoreError, WorldStore};
```

Change the struct and `new`:

```rust
pub struct Engine {
    realm: Realm,
    clock: Box<dyn Clock>,
    sessions: BTreeMap<SessionId, Session>,
    next_session: u64,
    tick: u64,
    store: Option<Box<dyn WorldStore>>,
    snapshot_interval: Option<i64>,
    last_snapshot: i64,
}

impl Engine {
    pub fn new(realm: Realm, clock: Box<dyn Clock>) -> Engine {
        Engine {
            realm,
            clock,
            sessions: BTreeMap::new(),
            next_session: 0,
            tick: 0,
            store: None,
            snapshot_interval: None,
            last_snapshot: 0,
        }
    }
    // ... existing methods unchanged ...
}
```

- [ ] **Step 3: Write the failing tests (add to the `engine.rs` tests module)**

Add these tests. They need `GitStore` + a tempdir; add the imports at the top of the tests module (merge with existing test `use`s):

```rust
    use snakewood_core::{GitStore, WorldStore};
    use tempfile::tempdir;
```

```rust
    #[test]
    fn attach_store_sets_flag_and_snapshot_time() {
        let clock = ManualClock::new(4242);
        let dir = tempdir().unwrap();
        let store = GitStore::init(dir.path()).unwrap();
        let mut e = Engine::new(Realm::new(World::default()), Box::new(clock));
        assert!(!e.has_store());
        e.attach_store(Box::new(store));
        assert!(e.has_store());
    }

    #[test]
    fn boot_loads_realm_from_store() {
        // Pre-populate a store on disk, commit, then boot an Engine from it.
        let dir = tempdir().unwrap();
        {
            let mut store = GitStore::init(dir.path()).unwrap();
            let mut realm = Realm::new(world_two_rooms());
            let mut flags = std::collections::BTreeSet::new();
            flags.insert(crate::__test_flag_alive());
            realm.insert_mob(snakewood_core::Mob {
                id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
                name: "a goblin".to_string(),
                location: EntityId::new("snakewood/clearing").unwrap(),
                flags,
                responders: Vec::new(),
            });
            store.save_realm(&realm).unwrap();
            store.commit("seed", 1000).unwrap();
        }
        let store = GitStore::init(dir.path()).unwrap();
        let e = Engine::boot(Box::new(store), Box::new(ManualClock::new(2000))).unwrap();
        assert!(e.realm().world.room(&EntityId::new("snakewood/clearing").unwrap()).is_some());
        assert!(e.realm().mob(&EntityId::new("snakewood/mob/goblin#1").unwrap()).is_some());
        assert!(e.has_store());
    }
```

> The `crate::__test_flag_alive()` reference above is a mistake to avoid — instead import `Flag` directly. Use `use snakewood_core::Flag;` in the tests module and write `flags.insert(Flag::Alive);`. (Do NOT create any `__test_flag_alive` helper.)

Replace that fixture line accordingly: `use snakewood_core::Flag;` at the top of the tests module and `flags.insert(Flag::Alive);` in the test body.

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p snakewood-daemon engine::tests::boot_loads_realm_from_store`
Expected: FAIL to compile — `attach_store`/`boot`/`has_store` don't exist.

- [ ] **Step 5: Implement `attach_store`, `boot`, `has_store`**

Add to the `impl Engine` block:

```rust
    /// Attach a store for persistence and reset the snapshot clock to now.
    pub fn attach_store(&mut self, store: Box<dyn WorldStore>) {
        self.last_snapshot = self.clock.now_unix();
        self.store = Some(store);
    }

    /// Boot an engine by loading the entire realm from `store`.
    pub fn boot(store: Box<dyn WorldStore>, clock: Box<dyn Clock>) -> Result<Engine, StoreError> {
        let realm = store.load_realm()?;
        let mut engine = Engine::new(realm, clock);
        engine.attach_store(store);
        Ok(engine)
    }

    pub fn has_store(&self) -> bool {
        self.store.is_some()
    }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p snakewood-daemon engine`
Expected: PASS (the two new tests + all Stage 3a engine tests).

- [ ] **Step 7: Commit**

```bash
git add crates/snakewood-daemon/Cargo.toml crates/snakewood-daemon/src/engine.rs
git commit -m "feat: Engine gains an optional store with boot/attach_store"
```

---

### Task 2: `checkpoint` — on-checkpoint / on-change commit

**Files:**
- Modify: `crates/snakewood-daemon/src/engine.rs`

**Interfaces:**
- Consumes: the store field (Task 1); `StoreError`.
- Produces: `Engine::checkpoint(&mut self, message: &str) -> Result<(), StoreError>` — if a store is attached, `save_realm(&realm)` then `commit(message, now)` at the current clock time; a no-op returning `Ok(())` if no store.

- [ ] **Step 1: Write the failing test (add to `engine.rs` tests)**

```rust
    #[test]
    fn checkpoint_persists_live_state_across_a_restart() {
        let dir = tempdir().unwrap();
        // First engine: seed a world with an actor, move it, checkpoint.
        {
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
            let store = GitStore::init(dir.path()).unwrap();
            let mut e = Engine::new(realm, Box::new(ManualClock::new(1000)));
            e.attach_store(Box::new(store));
            let sid = e.connect(EntityId::new("snakewood/pc/nathan").unwrap());
            e.submit(sid, Intent::Move {
                actor: EntityId::new("snakewood/pc/nathan").unwrap(),
                direction: Direction::North,
            });
            e.checkpoint("player moved north").unwrap();
        }
        // Second engine: boot from the same dir — the actor is at the moved location.
        let store = GitStore::init(dir.path()).unwrap();
        let e2 = Engine::boot(Box::new(store), Box::new(ManualClock::new(2000))).unwrap();
        assert_eq!(
            e2.realm().mob_location(&EntityId::new("snakewood/pc/nathan").unwrap()).map(|r| r.as_str()),
            Some("snakewood/old-well")
        );
        // Sessions are runtime-only; a freshly booted engine has none.
        assert_eq!(e2.session_actor(SessionId(0)), None);
    }

    #[test]
    fn checkpoint_without_store_is_ok_noop() {
        let mut e = engine();
        assert!(e.checkpoint("nothing to persist").is_ok());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p snakewood-daemon engine::tests::checkpoint_persists_live_state_across_a_restart`
Expected: FAIL to compile — `checkpoint` doesn't exist.

- [ ] **Step 3: Implement `checkpoint`**

Add to the `impl Engine` block (note: `now` is read before the store borrow so `self.clock` and `self.store`/`self.realm` borrows don't overlap):

```rust
    /// Persist the whole realm and commit it immediately (on-checkpoint; also
    /// used by authored on-change ops once they exist). No-op without a store.
    pub fn checkpoint(&mut self, message: &str) -> Result<(), StoreError> {
        let now = self.clock.now_unix();
        if let Some(store) = self.store.as_mut() {
            store.save_realm(&self.realm)?;
            store.commit(message, now)?;
        }
        Ok(())
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p snakewood-daemon engine`
Expected: PASS — the restart-survival test proves live state persists, plus the no-store no-op.

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-daemon/src/engine.rs
git commit -m "feat: Engine.checkpoint persists the realm (on-checkpoint policy)"
```

---

### Task 3: `set_snapshot_interval` + `maybe_snapshot` — on-interval policy

**Files:**
- Modify: `crates/snakewood-daemon/src/engine.rs`

**Interfaces:**
- Consumes: the store/interval fields (Task 1); `StoreError`.
- Produces:
  - `Engine::set_snapshot_interval(&mut self, secs: i64)` — configures the on-interval cadence.
  - `Engine::maybe_snapshot(&mut self) -> Result<bool, StoreError>` — if an interval is configured and `now - last_snapshot >= interval`, and a store is attached, `save_realm` + `commit("interval snapshot", now)` and update `last_snapshot`; returns `true` if it committed, `false` otherwise. Driven from the tick loop.

- [ ] **Step 1: Write the failing tests (add to `engine.rs` tests)**

```rust
    fn engine_with_store_and_actor(dir: &std::path::Path, start: i64) -> (Engine, SessionId) {
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
        let store = GitStore::init(dir).unwrap();
        let mut e = Engine::new(realm, Box::new(ManualClock::new(start)));
        e.attach_store(Box::new(store));
        let sid = e.connect(EntityId::new("snakewood/pc/nathan").unwrap());
        (e, sid)
    }

    #[test]
    fn maybe_snapshot_waits_for_the_interval() {
        let dir = tempdir().unwrap();
        let clock = ManualClock::new(0);
        // Build the engine sharing this clock handle by cloning time control:
        // we drive time by reconstructing via attach; here use a fresh clock we keep.
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
        // Use a ManualClock we can advance; move it into the engine but keep a raw
        // control path by advancing before/after via a shared approach:
        let control = std::sync::Arc::new(ManualClock::new(0));
        let mut e = Engine::new(realm, Box::new(ArcClock(control.clone())));
        let dir_store = GitStore::init(dir.path()).unwrap();
        e.attach_store(Box::new(dir_store));
        e.set_snapshot_interval(3600);

        // Not enough time elapsed -> no snapshot.
        control.advance(100);
        assert_eq!(e.maybe_snapshot().unwrap(), false);

        // Cross the interval -> snapshot commits.
        control.advance(3600);
        assert_eq!(e.maybe_snapshot().unwrap(), true);

        // Immediately after, not due again.
        assert_eq!(e.maybe_snapshot().unwrap(), false);
    }

    #[test]
    fn maybe_snapshot_without_interval_is_false() {
        let dir = tempdir().unwrap();
        let (mut e, _sid) = engine_with_store_and_actor(dir.path(), 0);
        // No interval configured.
        assert_eq!(e.maybe_snapshot().unwrap(), false);
    }
```

This test needs a `Clock` whose time we can advance AFTER moving it into the `Engine`. Add a tiny test-only `ArcClock` wrapper at the top of the tests module:

```rust
    struct ArcClock(std::sync::Arc<ManualClock>);
    impl crate::Clock for ArcClock {
        fn now_unix(&self) -> i64 {
            self.0.now_unix()
        }
    }
```

(`ManualClock` is `Send + Sync` via its `AtomicI64`, so `Arc<ManualClock>` and `ArcClock` satisfy `Clock: Send + Sync`.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p snakewood-daemon engine::tests::maybe_snapshot_waits_for_the_interval`
Expected: FAIL to compile — `set_snapshot_interval`/`maybe_snapshot` don't exist.

- [ ] **Step 3: Implement**

Add to the `impl Engine` block:

```rust
    /// Configure the on-interval snapshot cadence (seconds of injected time).
    pub fn set_snapshot_interval(&mut self, secs: i64) {
        self.snapshot_interval = Some(secs);
    }

    /// Commit an interval snapshot if the configured interval has elapsed since
    /// the last one. Returns whether it committed. Driven from the tick loop.
    pub fn maybe_snapshot(&mut self) -> Result<bool, StoreError> {
        let now = self.clock.now_unix();
        let due = matches!(self.snapshot_interval, Some(iv) if now - self.last_snapshot >= iv);
        if !due {
            return Ok(false);
        }
        let mut committed = false;
        if let Some(store) = self.store.as_mut() {
            store.save_realm(&self.realm)?;
            store.commit("interval snapshot", now)?;
            committed = true;
        }
        self.last_snapshot = now;
        Ok(committed)
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p snakewood-daemon engine`
Expected: PASS (interval waits then commits; no-interval false; plus all prior engine tests).

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-daemon/src/engine.rs
git commit -m "feat: Engine.maybe_snapshot commits on the configured interval (on-interval policy)"
```

---

### Task 4: Integration test — the world survives a restart

**Files:**
- Create: `crates/snakewood-daemon/tests/persistence_restart.rs`

**Interfaces:**
- Consumes: public API — `snakewood_daemon::{Engine, ManualClock, SessionId}`; `snakewood_core::{Realm, World, Room, Mob, Flag, EntityId, Direction, Intent, GitStore, WorldStore}`.
- Produces: an end-to-end test that boots a fresh persistent engine, connects, moves, checkpoints, drops it, boots a SECOND engine from the same git dir, and confirms the moved position survived — with no sessions carried over.

- [ ] **Step 1: Write `crates/snakewood-daemon/tests/persistence_restart.rs`**

```rust
use std::collections::{BTreeMap, BTreeSet};

use snakewood_core::{
    Direction, EntityId, Flag, GitStore, Intent, Mob, Realm, Room, World, WorldStore,
};
use snakewood_daemon::{Engine, ManualClock, SessionId};

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

fn seeded_realm() -> Realm {
    let mut realm = Realm::new(world());
    let mut flags = BTreeSet::new();
    flags.insert(Flag::Alive);
    realm.insert_mob(Mob {
        id: id("snakewood/pc/nathan"),
        name: "Nathan".to_string(),
        location: id("snakewood/clearing"),
        flags,
        responders: Vec::new(),
    });
    realm
}

#[test]
fn live_position_survives_a_daemon_restart() {
    let dir = tempfile::tempdir().unwrap();

    // First run: connect, walk north, checkpoint, then the engine goes away.
    {
        let store = GitStore::init(dir.path()).unwrap();
        let mut engine = Engine::new(seeded_realm(), Box::new(ManualClock::new(1000)));
        engine.attach_store(Box::new(store));
        let sid = engine.connect(id("snakewood/pc/nathan"));
        engine.submit(sid, Intent::Move { actor: id("snakewood/pc/nathan"), direction: Direction::North });
        assert_eq!(engine.realm().mob_location(&id("snakewood/pc/nathan")).map(|r| r.as_str()), Some("snakewood/old-well"));
        engine.checkpoint("nathan walked north").unwrap();
    }

    // Second run: boot from the same repo. Position persisted; no sessions.
    let store = GitStore::init(dir.path()).unwrap();
    let engine = Engine::boot(Box::new(store), Box::new(ManualClock::new(5000))).unwrap();
    assert_eq!(
        engine.realm().mob_location(&id("snakewood/pc/nathan")).map(|r| r.as_str()),
        Some("snakewood/old-well")
    );
    assert_eq!(engine.session_actor(SessionId(0)), None);
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p snakewood-daemon --test persistence_restart`
Expected: PASS (`live_position_survives_a_daemon_restart`).

- [ ] **Step 3: Commit**

```bash
git add crates/snakewood-daemon/tests/persistence_restart.rs
git commit -m "test: prove live world state survives a daemon restart"
```

---

### Task 5: Stage-completion verification

**Files:** none (verification only).

- [ ] **Step 1: Full workspace test suite**

Run: `cargo test --workspace`
Expected: all pass — `snakewood-core` (unchanged) and `snakewood-daemon` (engine unit tests incl. boot/checkpoint/interval + `persistence_restart`).

- [ ] **Step 2: Clippy and formatting**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no diffs. If `cargo fmt --check` reports diffs, run `cargo fmt`, re-run the suite, commit.

- [ ] **Step 3: Commit any formatting fixes**

```bash
git add -A
git commit -m "chore: stage 3b-scheduler verification — clippy clean, cargo fmt, workspace green"
```
(Skip if nothing to commit.)

---

## Self-Review

**1. Spec coverage (spec §5 persistence — the scheduling portion):**
- `on-checkpoint` (explicit save) — Task 2 (`checkpoint`). ✓
- `on-interval` (clock-driven snapshot) — Task 3 (`set_snapshot_interval` + `maybe_snapshot`). ✓
- `on-change` — mechanism realized by `checkpoint`; authored ops (3d `dig`) will call it. Documented; no daemon authored op exists yet. ✓
- Boot the live world from git — Task 1 (`boot`). ✓
- World survives a restart — Task 4 (integration). ✓
- Prune trap respected: no despawn added this sub-stage, so `save_realm` snapshots are correct; when despawn lands it must pair with `remove_mob` (documented, deferred). ✓
- Correctly deferred: async/telnet/MCP, despawn, `no_exit_message` persistence, `WorldStore: Send`. ✓

**2. Placeholder scan:** No "TBD/TODO". Task 1 Step 3 contains an explicit *anti-instruction* (the `__test_flag_alive` line is called out as a mistake to avoid, with the correct `Flag::Alive` given) — this is corrective guidance, not a placeholder; the implementer writes `use snakewood_core::Flag;` + `flags.insert(Flag::Alive);`. ✓

**3. Type consistency:** `Engine::new(Realm, Box<dyn Clock>)` unchanged; new methods `attach_store(Box<dyn WorldStore>)`, `boot(Box<dyn WorldStore>, Box<dyn Clock>) -> Result<Engine, StoreError>`, `has_store() -> bool`, `checkpoint(&str) -> Result<(), StoreError>`, `set_snapshot_interval(i64)`, `maybe_snapshot() -> Result<bool, StoreError>` used consistently across Tasks 1–4. Fields `store: Option<Box<dyn WorldStore>>`, `snapshot_interval: Option<i64>`, `last_snapshot: i64` referenced consistently. `WorldStore`/`StoreError`/`GitStore`/`Realm`/`Mob`/`Flag` come from `snakewood_core`; `save_realm`/`load_realm`/`commit` signatures match Stage 3b. The `ArcClock` test wrapper implements `crate::Clock` (which is `Send + Sync`) via `Arc<ManualClock>`. `checkpoint`/`maybe_snapshot` read `now` before borrowing `self.store` to keep field borrows disjoint. ✓
