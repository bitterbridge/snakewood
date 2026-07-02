# Snakewood M1 Stage 3b — Persistence Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist and reload the entire live world — authored rooms plus live `mobs` and global `rules` — through the version-controlled `WorldStore`, with commits that correctly capture deletions, so the daemon can boot a `Realm` from git and later checkpoint it back.

**Architecture:** Generalize the RON serializer to any serde type, then extend `WorldStore` with per-entity `mob`/`rules` persistence following the spec's `state/` (live) vs `world/` (authored) split. Two default trait methods — `load_realm`/`save_realm` — compose the per-entity operations so callers persist a whole `Realm` in one call. `GitStore::commit` is fixed to stage deletions/renames (not just additions), and a clone-and-reload test proves committed git *objects* (not just the working tree) contain the complete world. All work is in `snakewood-core`; the commit *scheduler* and its policies are a separate follow-on (Stage 3b-scheduler).

**Tech Stack:** Rust (edition 2021), existing `snakewood-core` (`serde`, `ron`, `git2`, `walkdir`, `tempfile` dev-dep). No new dependencies.

## Global Constraints

- **Language:** Rust, edition 2021. All work in `snakewood-core`. No new dependencies.
- **On-disk layout:** RON, one file per authored/live entity. Authored → `world/`; live instances → `state/`. Rooms: `world/<zone>/rooms/<name>.ron` (existing). Mobs (live): `state/<zone>/mobs/<name>.ron`. Rules (authored, global): a single `world/rules.ron` holding the `Vec<Rule>`.
- **Canonical & deterministic serialization:** same value → byte-identical output. `BTreeMap`/`BTreeSet` only, never `HashMap`. The shared `pretty_config()` (`struct_names(true)`, 4-space indent) governs all RON.
- **Storage behind `WorldStore`:** core logic never touches fs/git directly; only `GitStore`/`MemoryStore` do.
- **Injected time:** `commit` takes `epoch_seconds: i64` (already the case); no wall-clock reads.
- **`World` FROZEN** (Stage 1's `world.rs` and its round-trip/golden tests stay green); `snakewood-core`'s existing public API is only extended additively (the `WorldStore` trait gains methods — both impls must implement the new required ones).
- **Identity:** instance ids are `proto-id#serial` (e.g. `snakewood/mob/goblin#1`); `EntityId` allows `#`, and `id.zone()`/`id.name()` split on the first `/`.
- **Deferred (do NOT build):** the commit scheduler / policy engine (`on-change`/`on-interval`/`on-checkpoint` timing) and any `Engine`/daemon wiring — that's Stage 3b-scheduler. Prototypes/item entities, stale-file pruning on full snapshot.

---

### Task 1: Generic RON serialize/deserialize helpers

**Files:**
- Modify: `crates/snakewood-core/src/serialize.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: existing `pretty_config`, `Room`.
- Produces:
  - `pub fn to_ron<T: serde::Serialize>(value: &T) -> String` — canonical pretty RON for any serde type.
  - `pub fn from_ron<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, ron::error::SpannedError>`.
  - `room_to_ron`/`room_from_ron` retained, now delegating to `to_ron`/`from_ron` (behavior unchanged).
  - Re-export `to_ron`, `from_ron` from the crate root.

- [ ] **Step 1: Write the failing test in `serialize.rs`**

Add to the `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn generic_ron_round_trips_a_vec() {
        let v: Vec<u32> = vec![3, 1, 2];
        let text = to_ron(&v);
        let back: Vec<u32> = from_ron(&text).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn room_helpers_match_generic() {
        let room = clearing();
        assert_eq!(room_to_ron(&room), to_ron(&room));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p snakewood-core serialize::tests::generic_ron_round_trips_a_vec`
Expected: FAIL to compile — `to_ron`/`from_ron` don't exist.

- [ ] **Step 3: Implement the generic helpers and delegate the room helpers**

Replace the two `pub fn room_*` functions in `crates/snakewood-core/src/serialize.rs` with:

```rust
/// Serialize any value to canonical pretty RON.
pub fn to_ron<T: serde::Serialize>(value: &T) -> String {
    ron::ser::to_string_pretty(value, pretty_config())
        .expect("serialization is infallible for our field types")
}

/// Parse any value from RON text.
pub fn from_ron<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, ron::error::SpannedError> {
    ron::from_str(s)
}

/// Serialize a room to canonical pretty RON.
pub fn room_to_ron(room: &Room) -> String {
    to_ron(room)
}

/// Parse a room from RON text.
pub fn room_from_ron(s: &str) -> Result<Room, ron::error::SpannedError> {
    from_ron(s)
}
```

- [ ] **Step 4: Re-export from `lib.rs`**

Change the serialize re-export line in `crates/snakewood-core/src/lib.rs` to:

```rust
pub use serialize::{from_ron, room_from_ron, room_to_ron, to_ron};
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p snakewood-core serialize`
Expected: PASS — the two new tests plus the existing `round_trips_losslessly`, `serialization_is_deterministic`, `exit_keys_are_sorted_by_direction_order`.

- [ ] **Step 6: Commit**

```bash
git add crates/snakewood-core/src/serialize.rs crates/snakewood-core/src/lib.rs
git commit -m "feat: add generic to_ron/from_ron helpers"
```

---

### Task 2: Extend `WorldStore` with mob/rules persistence + `MemoryStore` impl + default `load_realm`/`save_realm`

**Files:**
- Modify: `crates/snakewood-core/src/store/mod.rs`
- Modify: `crates/snakewood-core/src/store/memory.rs`

**Interfaces:**
- Consumes: `Room`, `World`, `Realm`, `Mob`, `Rule`, `EntityId`.
- Produces — new required `WorldStore` methods (both impls must provide):
  - `fn save_mob(&mut self, mob: &Mob) -> Result<(), StoreError>;`
  - `fn remove_mob(&mut self, id: &EntityId) -> Result<(), StoreError>;` (no-op if absent)
  - `fn load_mobs(&self) -> Result<Vec<Mob>, StoreError>;`
  - `fn save_rules(&mut self, rules: &[Rule]) -> Result<(), StoreError>;`
  - `fn load_rules(&self) -> Result<Vec<Rule>, StoreError>;`
- Produces — default trait methods (composed, not overridden by impls):
  - `fn load_realm(&self) -> Result<Realm, StoreError>` — `Realm::new(load_all())` + insert each `load_mobs()` + `rules = load_rules()`.
  - `fn save_realm(&mut self, realm: &Realm) -> Result<(), StoreError>` — save every room, every mob, then the rules.
- `MemoryStore` gains `mobs: BTreeMap<EntityId, Mob>` and `rules: Vec<Rule>` fields implementing the new methods.

- [ ] **Step 1: Extend the trait in `crates/snakewood-core/src/store/mod.rs`**

Change the top `use` to `use crate::{EntityId, Mob, Realm, Room, Rule, World};` and add the new methods (required + default) to the `WorldStore` trait, after `commit_log`:

```rust
    /// Persist a single live mob instance (to `state/` in git-backed impls).
    fn save_mob(&mut self, mob: &Mob) -> Result<(), StoreError>;

    /// Remove a persisted mob by id (no-op if it isn't stored).
    fn remove_mob(&mut self, id: &EntityId) -> Result<(), StoreError>;

    /// Load all live mob instances.
    fn load_mobs(&self) -> Result<Vec<Mob>, StoreError>;

    /// Persist the global rule list (authored, `world/` in git-backed impls).
    fn save_rules(&mut self, rules: &[Rule]) -> Result<(), StoreError>;

    /// Load the global rule list (empty if none persisted).
    fn load_rules(&self) -> Result<Vec<Rule>, StoreError>;

    /// Load the entire live realm: authored rooms + live mobs + global rules.
    fn load_realm(&self) -> Result<Realm, StoreError> {
        let mut realm = Realm::new(self.load_all()?);
        for mob in self.load_mobs()? {
            realm.insert_mob(mob);
        }
        realm.rules = self.load_rules()?;
        Ok(realm)
    }

    /// Persist an entire realm: every room, every mob, and the rule list.
    fn save_realm(&mut self, realm: &Realm) -> Result<(), StoreError> {
        for room in realm.world.rooms.values() {
            self.save_room(room)?;
        }
        for mob in realm.mobs.values() {
            self.save_mob(mob)?;
        }
        self.save_rules(&realm.rules)?;
        Ok(())
    }
```

- [ ] **Step 2: Write failing tests + implement in `crates/snakewood-core/src/store/memory.rs`**

Change `MemoryStore` to hold mobs and rules, implement the new methods, and add tests. The struct + impl become (keep the existing `rooms`/`commits`/`next_commit` fields and their methods):

```rust
#[derive(Default)]
pub struct MemoryStore {
    rooms: BTreeMap<EntityId, Room>,
    mobs: BTreeMap<EntityId, Mob>,
    rules: Vec<Rule>,
    commits: Vec<String>,
    next_commit: u64,
}
```

Add the new imports at the top: `use crate::{EntityId, Mob, Room, Rule, World};` (merge with the existing use line). Then add these methods to the `impl WorldStore for MemoryStore` block:

```rust
    fn save_mob(&mut self, mob: &Mob) -> Result<(), StoreError> {
        self.mobs.insert(mob.id.clone(), mob.clone());
        Ok(())
    }

    fn remove_mob(&mut self, id: &EntityId) -> Result<(), StoreError> {
        self.mobs.remove(id);
        Ok(())
    }

    fn load_mobs(&self) -> Result<Vec<Mob>, StoreError> {
        Ok(self.mobs.values().cloned().collect())
    }

    fn save_rules(&mut self, rules: &[Rule]) -> Result<(), StoreError> {
        self.rules = rules.to_vec();
        Ok(())
    }

    fn load_rules(&self) -> Result<Vec<Rule>, StoreError> {
        Ok(self.rules.clone())
    }
```

Add tests to the memory tests module:

```rust
    #[test]
    fn saves_loads_and_removes_mobs() {
        use crate::{Flag, Mob};
        use std::collections::BTreeSet;
        let mut store = MemoryStore::new();
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        let mob = Mob {
            id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
            name: "a goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        };
        store.save_mob(&mob).unwrap();
        assert_eq!(store.load_mobs().unwrap(), vec![mob.clone()]);
        store.remove_mob(&mob.id).unwrap();
        assert!(store.load_mobs().unwrap().is_empty());
        // removing an absent mob is a no-op
        store.remove_mob(&mob.id).unwrap();
    }

    #[test]
    fn load_realm_composes_rooms_mobs_rules() {
        use crate::Mob;
        use std::collections::BTreeSet;
        let mut store = MemoryStore::new();
        store.save_room(&clearing()).unwrap();
        store
            .save_mob(&Mob {
                id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
                name: "a goblin".to_string(),
                location: EntityId::new("snakewood/clearing").unwrap(),
                flags: BTreeSet::new(),
                responders: Vec::new(),
            })
            .unwrap();
        let realm = store.load_realm().unwrap();
        assert!(realm.world.room(&EntityId::new("snakewood/clearing").unwrap()).is_some());
        assert!(realm.mob(&EntityId::new("snakewood/mob/goblin#1").unwrap()).is_some());
        // no_exit_message defaulted via Realm::new
        assert_eq!(realm.no_exit_message, "You see no exit in that direction.");
    }
```

(The existing `clearing()` helper in `memory.rs` tests builds a room; if it does not exist there, add one identical to the `clearing()` used elsewhere — a room with id `snakewood/clearing`.)

- [ ] **Step 3: Run tests**

Run: `cargo test -p snakewood-core store::memory`
Expected: PASS — the two new tests plus the existing `saves_and_loads_a_room`, `records_commit_messages_in_order`.

- [ ] **Step 4: Confirm the whole core crate still builds (GitStore must implement the new required methods — Task 3; until then the crate will NOT compile)**

Run: `cargo build -p snakewood-core 2>&1 | head -20`
Expected: **compile errors** in `git.rs` — `GitStore` doesn't yet implement `save_mob`/`remove_mob`/`load_mobs`/`save_rules`/`load_rules`. This is expected; Task 3 fixes it. Do NOT add stubs to `git.rs` in this task.

> Because the trait gains required methods, `MemoryStore` compiles but `GitStore` will not until Task 3. To keep this task's commit green on its own, this task's Step 3 runs ONLY the `store::memory` tests (which compile the lib including git.rs? — no: `cargo test` compiles the whole crate). To avoid an uncompilable commit, IMPLEMENT TASK 3 BEFORE COMMITTING. See Step 5.

- [ ] **Step 5: Commit policy for this task**

Because `snakewood-core` will not compile until `GitStore` also implements the new methods, **do not commit at the end of Task 2 alone.** Instead: complete Task 2's edits, then immediately proceed to Task 3, and make ONE commit covering both (the trait extension + both impls). Stage Task 2's files now, but run the commit in Task 3 Step 5:

```bash
git add crates/snakewood-core/src/store/mod.rs crates/snakewood-core/src/store/memory.rs
# (commit happens in Task 3 after GitStore compiles)
```

---

### Task 3: `GitStore` implementation of mob/rules persistence

**Files:**
- Modify: `crates/snakewood-core/src/store/git.rs`

**Interfaces:**
- Consumes: `to_ron`/`from_ron` (Task 1); the new `WorldStore` methods (Task 2); `Mob`, `Rule`, `EntityId`.
- Produces: `GitStore` implementations of `save_mob`, `remove_mob`, `load_mobs`, `save_rules`, `load_rules`, with these on-disk paths:
  - mob: `root/state/<zone>/mobs/<name>.ron` (`<zone>` = `id.zone()`, `<name>` = `id.name()`).
  - rules: `root/world/rules.ron` (single file holding the `Vec<Rule>`).

- [ ] **Step 1: Add the mob path helper and the five methods to `git.rs`**

Add the import at the top of `crates/snakewood-core/src/store/git.rs` (merge into the existing `use crate::{...}` line): `use crate::{from_ron, room_from_ron, room_to_ron, to_ron, EntityId, Mob, Realm, Room, Rule, World};` — keep whatever is already imported and add `from_ron, to_ron, EntityId, Mob, Rule` as needed.

Add a `mob_path` helper next to the existing `room_path`:

```rust
    fn mob_path(&self, mob: &Mob) -> PathBuf {
        self.root
            .join("state")
            .join(mob.id.zone())
            .join("mobs")
            .join(format!("{}.ron", mob.id.name()))
    }

    fn mob_path_for_id(&self, id: &EntityId) -> PathBuf {
        self.root
            .join("state")
            .join(id.zone())
            .join("mobs")
            .join(format!("{}.ron", id.name()))
    }

    fn rules_path(&self) -> PathBuf {
        self.root.join("world").join("rules.ron")
    }
```

Add these methods to the `impl WorldStore for GitStore` block:

```rust
    fn save_mob(&mut self, mob: &Mob) -> Result<(), StoreError> {
        let path = self.mob_path(mob);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        fs::write(&path, to_ron(mob)).map_err(io_err)?;
        Ok(())
    }

    fn remove_mob(&mut self, id: &EntityId) -> Result<(), StoreError> {
        let path = self.mob_path_for_id(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(e)),
        }
    }

    fn load_mobs(&self) -> Result<Vec<Mob>, StoreError> {
        let mut mobs = Vec::new();
        let state_dir = self.root.join("state");
        if !state_dir.exists() {
            return Ok(mobs);
        }
        for entry in WalkDir::new(&state_dir).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("ron") {
                continue;
            }
            let text = fs::read_to_string(path).map_err(io_err)?;
            let mob: Mob = from_ron(&text).map_err(|e| StoreError::Parse(e.to_string()))?;
            mobs.push(mob);
        }
        // Deterministic order by id.
        mobs.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        Ok(mobs)
    }

    fn save_rules(&mut self, rules: &[Rule]) -> Result<(), StoreError> {
        let path = self.rules_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        fs::write(&path, to_ron(rules)).map_err(io_err)?;
        Ok(())
    }

    fn load_rules(&self) -> Result<Vec<Rule>, StoreError> {
        let path = self.rules_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&path).map_err(io_err)?;
        from_ron(&text).map_err(|e| StoreError::Parse(e.to_string()))
    }
```

- [ ] **Step 2: Add a GitStore round-trip test for mobs and rules**

Add to `git.rs` tests:

```rust
    #[test]
    fn realm_round_trips_through_git() {
        use crate::fabric::{Outcome, Rule, Trigger};
        use crate::{Flag, Mob};
        use std::collections::BTreeSet;

        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();

        let mut realm = crate::Realm::new({
            let mut w = World::default();
            w.insert_room(clearing());
            w
        });
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        realm.insert_mob(Mob {
            id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
            name: "a goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        });
        realm.rules.push(Rule {
            on: Trigger::AnyMove,
            require: Vec::new(),
            effects: Vec::new(),
            outcome: Outcome::Allow,
            priority: 0,
        });

        store.save_realm(&realm).unwrap();
        store.commit("save realm", 1_700_000_000).unwrap();

        let reloaded = GitStore::init(dir.path()).unwrap().load_realm().unwrap();
        assert_eq!(reloaded.world, realm.world);
        assert_eq!(reloaded.mobs, realm.mobs);
        assert_eq!(reloaded.rules, realm.rules);
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p snakewood-core store::git`
Expected: PASS — the new `realm_round_trips_through_git` plus the existing git tests.

- [ ] **Step 4: Run the whole core suite to confirm the crate compiles and is green**

Run: `cargo test -p snakewood-core`
Expected: all pass (Task 2's memory tests now compile too, since `GitStore` implements the required methods).

- [ ] **Step 5: Commit (covers Task 2 + Task 3)**

```bash
git add crates/snakewood-core/src/store/mod.rs crates/snakewood-core/src/store/memory.rs crates/snakewood-core/src/store/git.rs
git commit -m "feat: persist and load mobs and rules; load_realm/save_realm"
```

---

### Task 4: Deletion-safe commit

**Files:**
- Modify: `crates/snakewood-core/src/store/git.rs`

**Interfaces:**
- Consumes: existing `commit`.
- Produces: `GitStore::commit` stages new AND modified AND deleted paths (so a `remove_mob` followed by `commit` actually drops the file from the tree). Achieved by following `index.add_all(["*"], …)` with `index.update_all(["*"], None)`.

- [ ] **Step 1: Write the failing deletion round-trip test in `git.rs`**

```rust
    #[test]
    fn commit_stages_deletions() {
        use crate::{Flag, Mob};
        use std::collections::BTreeSet;

        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        let goblin = Mob {
            id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
            name: "a goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        };
        store.save_mob(&goblin).unwrap();
        store.commit("spawn goblin", 1_700_000_000).unwrap();

        // Remove and commit the deletion.
        store.remove_mob(&goblin.id).unwrap();
        store.commit("goblin dies", 1_700_000_100).unwrap();

        // A fresh CLONE of the committed repo must not contain the goblin —
        // proving the deletion was staged into the tree, not just the working dir.
        let clone_dir = tempdir().unwrap();
        let repo_url = dir.path().to_str().unwrap();
        git2::Repository::clone(repo_url, clone_dir.path()).unwrap();
        let reloaded = GitStore::init(clone_dir.path()).unwrap().load_mobs().unwrap();
        assert!(reloaded.is_empty(), "deleted mob must be gone from committed tree: {reloaded:?}");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p snakewood-core store::git::tests::commit_stages_deletions`
Expected: FAIL — the cloned repo still contains the goblin file (current `add_all` doesn't stage the deletion), so `reloaded` is non-empty.

- [ ] **Step 3: Fix `commit` to stage deletions**

In `crates/snakewood-core/src/store/git.rs`, the `commit` method currently stages with a single `add_all`. Add an `update_all` right after it (which synchronizes tracked entries with the working dir, removing entries whose files are gone):

```rust
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .map_err(git_err)?;
        // add_all stages new + modified files but not deletions of tracked files;
        // update_all removes index entries whose working-tree file is gone.
        index.update_all(["*"].iter(), None).map_err(git_err)?;
        index.write().map_err(git_err)?;
```

Also update the existing `// NOTE: add_all ... must switch to update_all ...` comment (added in Stage 1) to reflect that deletions are now handled.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p snakewood-core store::git`
Expected: PASS — `commit_stages_deletions` now passes (cloned tree has no goblin) plus all prior git tests.

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-core/src/store/git.rs
git commit -m "fix: commit stages deletions via update_all"
```

---

### Task 5: Git-object-DB reload proof (clone round-trip)

**Files:**
- Create: `crates/snakewood-core/tests/realm_persistence.rs`

**Interfaces:**
- Consumes: crate public API — `snakewood_core::{Realm, World, Room, Mob, Flag, EntityId, Direction, GitStore, WorldStore}` and `snakewood_core::fabric::{Rule, Trigger, Outcome}`.
- Produces: an integration test that saves a full realm, commits, **clones the repo to a fresh directory**, and loads the realm from the clone — proving the committed git objects (not merely the working tree) contain the complete world.

- [ ] **Step 1: Write `crates/snakewood-core/tests/realm_persistence.rs`**

```rust
use std::collections::{BTreeMap, BTreeSet};

use snakewood_core::fabric::{Outcome, Rule, Trigger};
use snakewood_core::{
    Direction, EntityId, Flag, GitStore, Mob, Realm, Room, World, WorldStore,
};

fn id(s: &str) -> EntityId {
    EntityId::new(s).unwrap()
}

fn sample_realm() -> Realm {
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
    let mut realm = Realm::new(world);
    let mut flags = BTreeSet::new();
    flags.insert(Flag::Alive);
    flags.insert(Flag::Conscious);
    realm.insert_mob(Mob {
        id: id("snakewood/mob/goblin#1"),
        name: "a goblin".to_string(),
        location: id("snakewood/clearing"),
        flags,
        responders: Vec::new(),
    });
    realm.rules.push(Rule {
        on: Trigger::AnyMove,
        require: Vec::new(),
        effects: Vec::new(),
        outcome: Outcome::Allow,
        priority: 7,
    });
    realm
}

#[test]
fn realm_survives_a_git_clone() {
    let src = tempfile::tempdir().unwrap();
    let mut store = GitStore::init(src.path()).unwrap();
    let realm = sample_realm();
    store.save_realm(&realm).unwrap();
    store.commit("initial realm", 1_700_000_000).unwrap();

    // Clone the committed repo into a fresh directory and load from the clone.
    let dst = tempfile::tempdir().unwrap();
    git2::Repository::clone(src.path().to_str().unwrap(), dst.path()).unwrap();
    let reloaded = GitStore::init(dst.path()).unwrap().load_realm().unwrap();

    assert_eq!(reloaded.world, realm.world);
    assert_eq!(reloaded.mobs, realm.mobs);
    assert_eq!(reloaded.rules, realm.rules);
    assert_eq!(reloaded.no_exit_message, realm.no_exit_message);
}
```

> This test uses `git2` and `tempfile` directly. `git2` is a normal dependency of `snakewood-core` and `tempfile` is a dev-dependency, both already available to integration tests. If `git2` is not resolvable from the test crate, add `git2 = { workspace = true }` to `snakewood-core`'s `[dev-dependencies]` (it is already a normal dependency, so this should not be necessary).

- [ ] **Step 2: Run the test**

Run: `cargo test -p snakewood-core --test realm_persistence`
Expected: PASS (`realm_survives_a_git_clone`).

- [ ] **Step 3: Commit**

```bash
git add crates/snakewood-core/tests/realm_persistence.rs
git commit -m "test: prove the full realm survives a git clone (committed objects complete)"
```

---

### Task 6: Stage-completion verification

**Files:** none (verification only).

- [ ] **Step 1: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: all pass — `snakewood-core` (Stage 1/2 + new persistence tests incl. `realm_persistence`) and `snakewood-daemon` (unchanged, still green). Confirm the Stage 1 room round-trip + golden tests are still green (`World` frozen).

- [ ] **Step 2: Clippy and formatting**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no diffs. If `cargo fmt --check` reports diffs, run `cargo fmt`, re-run the suite, commit.

- [ ] **Step 3: Commit any formatting fixes**

```bash
git add -A
git commit -m "chore: stage 3b verification — clippy clean, cargo fmt, workspace green"
```
(Skip if nothing to commit.)

---

## Self-Review

**1. Spec coverage (spec §5 persistence — the storage-layer portion):**
- `state/` (live) vs `world/` (authored) split — mobs → `state/<zone>/mobs/`, rules → `world/rules.ron`, rooms unchanged (Tasks 3). ✓
- Persist + load mobs and rules; boot-load a full `Realm` — Tasks 2–3 (`load_realm`/`save_realm`). ✓
- Deletion-safe commit (carry-forward: `add_all` didn't stage deletions) — Task 4 (`update_all`). ✓
- Git-object-DB reload proof (carry-forward: round-trip read the working tree, not git objects) — Tasks 4–5 (clone round-trip). ✓
- Generic serializer for non-room entities — Task 1. ✓
- Correctly deferred to Stage 3b-scheduler: the commit *scheduler* and `on-change`/`on-interval`/`on-checkpoint` timing, and any `Engine` wiring. Also deferred: stale-file pruning on full snapshot, prototype/item entities. ✓

**2. Placeholder scan:** No "TBD/TODO/implement later". Task 2's Step 4/5 deliberately document that the crate won't compile until Task 3 (a required-trait-method reality) and fold Tasks 2+3 into a single commit — this is an explicit, correct handling of the compile dependency, not a placeholder. ✓

**3. Type consistency:** `to_ron<T: Serialize>`/`from_ron<T: DeserializeOwned>` used consistently (Tasks 1, 3). New `WorldStore` methods (`save_mob`, `remove_mob`, `load_mobs`, `save_rules`, `load_rules`, default `load_realm`/`save_realm`) match between the trait (Task 2), `MemoryStore` (Task 2), and `GitStore` (Task 3). `Mob`/`Rule` field shapes match Stage 2 (`Mob { id, name, location, flags, responders }`; `Rule { on, require, effects, outcome, priority }`). Paths: mobs `state/<zone>/mobs/<name>.ron`, rules `world/rules.ron` — consistent across `git.rs` save/load. `Realm::new` sets `no_exit_message`, so `load_realm` yields the default message (asserted in Tasks 2 and 5). ✓
