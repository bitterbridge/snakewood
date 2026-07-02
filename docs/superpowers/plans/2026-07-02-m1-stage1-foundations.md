# Snakewood M1 Stage 1 — Foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the Snakewood Cargo workspace and prove the principal concern — a live world data model that serializes to canonical RON and round-trips losslessly through a git-backed, version-controlled store.

**Architecture:** A pure `snakewood-core` library holds the world data model (rooms, IDs, directions, world container) with no I/O. Serialization is canonical RON (deterministic field order, sorted maps) so diffs are meaningful. Persistence hides behind a `WorldStore` trait with two implementations: an in-memory store for fast tests and a git-backed store that writes one RON file per entity and records commits. Round-trip correctness is proven with property tests and golden-file snapshots.

**Tech Stack:** Rust (edition 2021), `serde` (derive), `ron` 0.8 (canonical text format), `git2` 0.19 (libgit2 bindings, vendored — no system dependency), `walkdir` 2 (directory traversal), `proptest` 1 (property testing, dev-dependency), `tempfile` 3 (temp dirs in tests, dev-dependency).

## Global Constraints

- **Language:** Rust, edition 2021, workspace `resolver = "2"`. Every crate is a workspace member.
- **No embedded scripting.** All world content is data. (Stage 1 has no behavior yet; this constraint governs later stages but is noted so no scripting hooks are introduced.)
- **Serialization is canonical and deterministic.** Same value → byte-identical output every time. Achieved via fixed struct field order + `BTreeMap` (sorted keys), never `HashMap`, in any serialized type.
- **On-disk format:** RON, **one file per authored entity**, grouped by zone: `world/<zone>/rooms/<name>.ron`.
- **Identity:** human-readable, namespaced string IDs (e.g. `snakewood/clearing`). Never UUIDs.
- **Storage is behind the `WorldStore` trait.** Core logic never touches the filesystem or git directly.
- **Time is injected, never read from the wall clock.** Commit timestamps are passed in as parameters (a `Clock` abstraction arrives in Stage 2). Tests pass fixed timestamps so commits are reproducible.
- **Test at the cheapest layer.** Prefer in-process unit and property tests; the round-trip is proven with `proptest` + golden files.

---

### Task 1: Workspace & core crate scaffold

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/snakewood-core/Cargo.toml`
- Create: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: a buildable workspace with the `snakewood-core` library crate; `snakewood_core` is importable.

- [ ] **Step 1: Write the workspace root `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = ["crates/snakewood-core"]

[workspace.package]
edition = "2021"
license = "MIT"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
ron = "0.8"
git2 = "0.19"
walkdir = "2"
proptest = "1"
tempfile = "3"
```

- [ ] **Step 2: Write `crates/snakewood-core/Cargo.toml`**

```toml
[package]
name = "snakewood-core"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
serde = { workspace = true }
ron = { workspace = true }
git2 = { workspace = true }
walkdir = { workspace = true }

[dev-dependencies]
proptest = { workspace = true }
tempfile = { workspace = true }
```

- [ ] **Step 3: Write a placeholder `crates/snakewood-core/src/lib.rs` with a smoke test**

```rust
//! snakewood-core: the pure, deterministic world model and storage layer.

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds() {
        assert_eq!(2 + 2, 4);
    }
}
```

- [ ] **Step 4: Run the build and test to verify the workspace compiles**

Run: `cargo test -p snakewood-core`
Expected: compiles; `smoke::workspace_builds` passes (1 passed).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/snakewood-core/Cargo.toml crates/snakewood-core/src/lib.rs
git commit -m "chore: scaffold cargo workspace and snakewood-core crate"
```

---

### Task 2: `Direction` enum

**Files:**
- Create: `crates/snakewood-core/src/direction.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub enum Direction` with variants `North, South, East, West, Up, Down`; derives `Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`. `Ord` is required so `Direction` can be a `BTreeMap` key with deterministic ordering.

- [ ] **Step 1: Write the failing test in `crates/snakewood-core/src/direction.rs`**

```rust
use serde::{Deserialize, Serialize};

/// A compass/vertical direction. Ordered so it can key a sorted map deterministically.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    North,
    South,
    East,
    West,
    Up,
    Down,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directions_sort_by_declaration_order() {
        let mut v = vec![Direction::Down, Direction::North, Direction::East];
        v.sort();
        assert_eq!(v, vec![Direction::North, Direction::East, Direction::Down]);
    }
}
```

- [ ] **Step 2: Wire the module into `lib.rs`**

Add to the top of `crates/snakewood-core/src/lib.rs`:

```rust
pub mod direction;

pub use direction::Direction;
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p snakewood-core direction`
Expected: PASS (`directions_sort_by_declaration_order`).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/direction.rs crates/snakewood-core/src/lib.rs
git commit -m "feat: add Direction enum"
```

---

### Task 3: `EntityId` newtype with validation

**Files:**
- Create: `crates/snakewood-core/src/id.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct EntityId(String)` — a validated, namespaced ID. Derives `Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash`. Serde-transparent (serializes as its inner string) so it is a plain string key on disk.
  - `EntityId::new(s: impl Into<String>) -> Result<EntityId, IdError>` — validates the format.
  - `EntityId::as_str(&self) -> &str`.
  - `EntityId::zone(&self) -> &str` — the first `/`-separated segment.
  - `EntityId::name(&self) -> &str` — everything after the first `/`.
  - `impl std::fmt::Display for EntityId`.
  - `pub enum IdError { Empty, InvalidChar(char), NoNamespace, LeadingOrTrailingSlash }` deriving `Debug, PartialEq`.

Validation rules: non-empty; only lowercase `a`–`z`, digits `0`–`9`, `/`, `_`, `-`; must contain at least one `/` (namespaced); no leading or trailing `/`.

- [ ] **Step 1: Write the failing tests in `crates/snakewood-core/src/id.rs`**

```rust
use std::fmt;

use serde::{Deserialize, Serialize};

/// A validated, human-readable, namespaced identifier, e.g. `snakewood/clearing`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct EntityId(String);

#[derive(Debug, PartialEq)]
pub enum IdError {
    Empty,
    InvalidChar(char),
    NoNamespace,
    LeadingOrTrailingSlash,
}

impl EntityId {
    pub fn new(s: impl Into<String>) -> Result<EntityId, IdError> {
        let s = s.into();
        if s.is_empty() {
            return Err(IdError::Empty);
        }
        if s.starts_with('/') || s.ends_with('/') {
            return Err(IdError::LeadingOrTrailingSlash);
        }
        for c in s.chars() {
            let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '/' | '_' | '-');
            if !ok {
                return Err(IdError::InvalidChar(c));
            }
        }
        if !s.contains('/') {
            return Err(IdError::NoNamespace);
        }
        Ok(EntityId(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn zone(&self) -> &str {
        self.0.split('/').next().unwrap_or(&self.0)
    }

    pub fn name(&self) -> &str {
        match self.0.split_once('/') {
            Some((_, rest)) => rest,
            None => &self.0,
        }
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_namespaced_id() {
        let id = EntityId::new("snakewood/clearing").unwrap();
        assert_eq!(id.as_str(), "snakewood/clearing");
        assert_eq!(id.zone(), "snakewood");
        assert_eq!(id.name(), "clearing");
    }

    #[test]
    fn name_keeps_deeper_segments() {
        let id = EntityId::new("snakewood/mob/goblin").unwrap();
        assert_eq!(id.zone(), "snakewood");
        assert_eq!(id.name(), "mob/goblin");
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(EntityId::new(""), Err(IdError::Empty));
    }

    #[test]
    fn rejects_missing_namespace() {
        assert_eq!(EntityId::new("clearing"), Err(IdError::NoNamespace));
    }

    #[test]
    fn rejects_uppercase() {
        assert_eq!(EntityId::new("Snakewood/clearing"), Err(IdError::InvalidChar('S')));
    }

    #[test]
    fn rejects_leading_or_trailing_slash() {
        assert_eq!(EntityId::new("/snakewood"), Err(IdError::LeadingOrTrailingSlash));
        assert_eq!(EntityId::new("snakewood/"), Err(IdError::LeadingOrTrailingSlash));
    }
}
```

- [ ] **Step 2: Wire the module into `lib.rs`**

Add to `crates/snakewood-core/src/lib.rs`:

```rust
pub mod id;

pub use id::{EntityId, IdError};
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p snakewood-core id`
Expected: PASS (6 tests in the `id` module).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/id.rs crates/snakewood-core/src/lib.rs
git commit -m "feat: add validated namespaced EntityId"
```

---

### Task 4: `Room` and `World` data types

**Files:**
- Create: `crates/snakewood-core/src/world.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: `EntityId` (Task 3), `Direction` (Task 2).
- Produces:
  - `pub struct Room { pub id: EntityId, pub name: String, pub description: String, pub exits: BTreeMap<Direction, EntityId> }` deriving `Serialize, Deserialize, Debug, Clone, PartialEq`.
  - `pub struct World { pub rooms: BTreeMap<EntityId, Room> }` deriving `Debug, Clone, PartialEq, Default`.
  - `World::insert_room(&mut self, room: Room)` — inserts keyed by `room.id`.
  - `World::room(&self, id: &EntityId) -> Option<&Room>`.

Note: `BTreeMap` (not `HashMap`) everywhere, so iteration and serialization are deterministic.

- [ ] **Step 1: Write the failing test in `crates/snakewood-core/src/world.rs`**

```rust
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Direction, EntityId};

/// A single authored place in the world. Exits are the ergonomic "sugar" form:
/// a direction mapping straight to a destination room id.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Room {
    pub id: EntityId,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub exits: BTreeMap<Direction, EntityId>,
}

/// The in-memory aggregate of all authored rooms.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct World {
    pub rooms: BTreeMap<EntityId, Room>,
}

impl World {
    pub fn insert_room(&mut self, room: Room) {
        self.rooms.insert(room.id.clone(), room);
    }

    pub fn room(&self, id: &EntityId) -> Option<&Room> {
        self.rooms.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clearing() -> Room {
        let mut exits = BTreeMap::new();
        exits.insert(Direction::North, EntityId::new("snakewood/old-well").unwrap());
        Room {
            id: EntityId::new("snakewood/clearing").unwrap(),
            name: "Snakewood Clearing".to_string(),
            description: "Gnarled snakewood trees ring a clearing of trampled grass.".to_string(),
            exits,
        }
    }

    #[test]
    fn insert_and_fetch_room() {
        let mut world = World::default();
        let room = clearing();
        let id = room.id.clone();
        world.insert_room(room.clone());
        assert_eq!(world.room(&id), Some(&room));
    }

    #[test]
    fn missing_room_is_none() {
        let world = World::default();
        let id = EntityId::new("snakewood/nowhere").unwrap();
        assert_eq!(world.room(&id), None);
    }
}
```

- [ ] **Step 2: Wire the module into `lib.rs`**

Add to `crates/snakewood-core/src/lib.rs`:

```rust
pub mod world;

pub use world::{Room, World};
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-core world`
Expected: PASS (`insert_and_fetch_room`, `missing_room_is_none`).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/world.rs crates/snakewood-core/src/lib.rs
git commit -m "feat: add Room and World data types"
```

---

### Task 5: Canonical RON serialization

**Files:**
- Create: `crates/snakewood-core/src/serialize.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: `Room` (Task 4).
- Produces:
  - `pub fn room_to_ron(room: &Room) -> String` — canonical pretty RON, deterministic.
  - `pub fn room_from_ron(s: &str) -> Result<Room, ron::error::SpannedError>`.
  - A module-private `pretty_config() -> ron::ser::PrettyConfig` used for all serialization so formatting is uniform.

- [ ] **Step 1: Write the failing tests in `crates/snakewood-core/src/serialize.rs`**

```rust
use ron::ser::PrettyConfig;

use crate::Room;

fn pretty_config() -> PrettyConfig {
    // Deterministic, human-readable output. Defaults already sort nothing
    // random; our types use BTreeMap + fixed field order, so output is stable.
    PrettyConfig::default()
        .struct_names(true)
        .indentor("    ".to_string())
}

/// Serialize a room to canonical pretty RON.
pub fn room_to_ron(room: &Room) -> String {
    ron::ser::to_string_pretty(room, pretty_config())
        .expect("Room serialization is infallible for our field types")
}

/// Parse a room from RON text.
pub fn room_from_ron(s: &str) -> Result<Room, ron::error::SpannedError> {
    ron::from_str(s)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{Direction, EntityId};

    fn clearing() -> Room {
        let mut exits = BTreeMap::new();
        exits.insert(Direction::North, EntityId::new("snakewood/old-well").unwrap());
        exits.insert(Direction::Down, EntityId::new("snakewood/root-cellar").unwrap());
        Room {
            id: EntityId::new("snakewood/clearing").unwrap(),
            name: "Snakewood Clearing".to_string(),
            description: "Gnarled snakewood trees ring a clearing.".to_string(),
            exits,
        }
    }

    #[test]
    fn round_trips_losslessly() {
        let room = clearing();
        let text = room_to_ron(&room);
        let parsed = room_from_ron(&text).unwrap();
        assert_eq!(parsed, room);
    }

    #[test]
    fn serialization_is_deterministic() {
        let room = clearing();
        assert_eq!(room_to_ron(&room), room_to_ron(&room));
    }

    #[test]
    fn exit_keys_are_sorted_by_direction_order() {
        // Down < North in declaration order, so Down must appear before North
        // regardless of insertion order.
        let room = clearing();
        let text = room_to_ron(&room);
        let down_pos = text.find("Down").expect("Down present");
        let north_pos = text.find("North").expect("North present");
        assert!(down_pos < north_pos, "exits must serialize in Direction order:\n{text}");
    }
}
```

- [ ] **Step 2: Wire the module into `lib.rs`**

Add to `crates/snakewood-core/src/lib.rs`:

```rust
pub mod serialize;

pub use serialize::{room_from_ron, room_to_ron};
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-core serialize`
Expected: PASS (`round_trips_losslessly`, `serialization_is_deterministic`, `exit_keys_are_sorted_by_direction_order`).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/serialize.rs crates/snakewood-core/src/lib.rs
git commit -m "feat: add canonical RON serialization for rooms"
```

---

### Task 6: `WorldStore` trait and in-memory store

**Files:**
- Create: `crates/snakewood-core/src/store/mod.rs`
- Create: `crates/snakewood-core/src/store/memory.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: `Room`, `World` (Task 4).
- Produces:
  - `pub enum StoreError { Io(String), Parse(String), Git(String) }` deriving `Debug`.
  - `pub struct CommitId(pub String)` deriving `Debug, Clone, PartialEq`.
  - `pub trait WorldStore`:
    - `fn save_room(&mut self, room: &Room) -> Result<(), StoreError>;`
    - `fn load_all(&self) -> Result<World, StoreError>;`
    - `fn commit(&mut self, message: &str, epoch_seconds: i64) -> Result<CommitId, StoreError>;` — `epoch_seconds` is the injected timestamp (no wall clock).
    - `fn commit_log(&self) -> Vec<String>;` — commit messages, oldest first (used by tests and later replay).
  - `pub struct MemoryStore` implementing `WorldStore`. `MemoryStore::new() -> MemoryStore`.

- [ ] **Step 1: Write the trait and errors in `crates/snakewood-core/src/store/mod.rs`**

```rust
use crate::{Room, World};

pub mod memory;

pub use memory::MemoryStore;

#[derive(Debug)]
pub enum StoreError {
    Io(String),
    Parse(String),
    Git(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitId(pub String);

/// A place authored world data is persisted and versioned. Core logic depends
/// only on this trait; implementations own all filesystem/git contact.
pub trait WorldStore {
    /// Persist a single room (one file per entity, in git-backed impls).
    fn save_room(&mut self, room: &Room) -> Result<(), StoreError>;

    /// Load the entire world from storage.
    fn load_all(&self) -> Result<World, StoreError>;

    /// Commit all pending saves with `message`, timestamped at `epoch_seconds`.
    fn commit(&mut self, message: &str, epoch_seconds: i64) -> Result<CommitId, StoreError>;

    /// Commit messages recorded so far, oldest first.
    fn commit_log(&self) -> Vec<String>;
}
```

- [ ] **Step 2: Write the failing test + `MemoryStore` in `crates/snakewood-core/src/store/memory.rs`**

```rust
use std::collections::BTreeMap;

use crate::store::{CommitId, StoreError, WorldStore};
use crate::{EntityId, Room, World};

/// An in-memory store for fast tests. "Commits" are recorded as snapshots so
/// behavior mirrors the git store closely enough for logic tests.
#[derive(Default)]
pub struct MemoryStore {
    rooms: BTreeMap<EntityId, Room>,
    commits: Vec<String>,
    next_commit: u64,
}

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }
}

impl WorldStore for MemoryStore {
    fn save_room(&mut self, room: &Room) -> Result<(), StoreError> {
        self.rooms.insert(room.id.clone(), room.clone());
        Ok(())
    }

    fn load_all(&self) -> Result<World, StoreError> {
        Ok(World {
            rooms: self.rooms.clone(),
        })
    }

    fn commit(&mut self, message: &str, _epoch_seconds: i64) -> Result<CommitId, StoreError> {
        self.commits.push(message.to_string());
        let id = CommitId(format!("mem-{}", self.next_commit));
        self.next_commit += 1;
        Ok(id)
    }

    fn commit_log(&self) -> Vec<String> {
        self.commits.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Direction;

    fn clearing() -> Room {
        let mut exits = BTreeMap::new();
        exits.insert(Direction::North, EntityId::new("snakewood/old-well").unwrap());
        Room {
            id: EntityId::new("snakewood/clearing").unwrap(),
            name: "Snakewood Clearing".to_string(),
            description: "A clearing.".to_string(),
            exits,
        }
    }

    #[test]
    fn saves_and_loads_a_room() {
        let mut store = MemoryStore::new();
        let room = clearing();
        store.save_room(&room).unwrap();
        let world = store.load_all().unwrap();
        assert_eq!(world.room(&room.id), Some(&room));
    }

    #[test]
    fn records_commit_messages_in_order() {
        let mut store = MemoryStore::new();
        store.commit("first", 1000).unwrap();
        store.commit("second", 2000).unwrap();
        assert_eq!(store.commit_log(), vec!["first".to_string(), "second".to_string()]);
    }
}
```

- [ ] **Step 3: Wire the module into `lib.rs`**

Add to `crates/snakewood-core/src/lib.rs`:

```rust
pub mod store;

pub use store::{CommitId, MemoryStore, StoreError, WorldStore};
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p snakewood-core store::memory`
Expected: PASS (`saves_and_loads_a_room`, `records_commit_messages_in_order`).

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-core/src/store crates/snakewood-core/src/lib.rs
git commit -m "feat: add WorldStore trait and in-memory store"
```

---

### Task 7: Git-backed store

**Files:**
- Create: `crates/snakewood-core/src/store/git.rs`
- Modify: `crates/snakewood-core/src/store/mod.rs`

**Interfaces:**
- Consumes: `WorldStore`, `StoreError`, `CommitId` (Task 6); `Room`, `World` (Task 4); `room_to_ron`, `room_from_ron` (Task 5).
- Produces:
  - `pub struct GitStore { root: PathBuf, repo: git2::Repository }`.
  - `GitStore::init(root: impl AsRef<Path>) -> Result<GitStore, StoreError>` — initializes (or opens) a git repo at `root`.
  - `impl WorldStore for GitStore`. On-disk path for a room: `root/world/<zone>/rooms/<name>.ron`, where `<zone>` = `id.zone()` and `<name>` = `id.name()` with `/` preserved as subdirectories. `load_all` walks `root/world/**/*.ron`, parses each as a `Room`, and keys it by its own `id` field (the file's contents are the source of truth). `commit` stages everything under `root` and creates a commit with a fixed author signature (`"Snakewood" <world@snakewood.local>`) at the injected `epoch_seconds`, chaining onto `HEAD` if it exists.

- [ ] **Step 1: Write the failing round-trip test + `GitStore` in `crates/snakewood-core/src/store/git.rs`**

```rust
use std::fs;
use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Repository, Signature, Time};
use walkdir::WalkDir;

use crate::store::{CommitId, StoreError, WorldStore};
use crate::{room_from_ron, room_to_ron, Room, World};

fn io_err<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Io(e.to_string())
}

fn git_err(e: git2::Error) -> StoreError {
    StoreError::Git(e.to_string())
}

/// A git-backed, version-controlled world store. One RON file per room under
/// `root/world/<zone>/rooms/<name>.ron`.
pub struct GitStore {
    root: PathBuf,
    repo: Repository,
}

impl GitStore {
    pub fn init(root: impl AsRef<Path>) -> Result<GitStore, StoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(io_err)?;
        let repo = match Repository::open(&root) {
            Ok(repo) => repo,
            Err(_) => Repository::init(&root).map_err(git_err)?,
        };
        Ok(GitStore { root, repo })
    }

    fn room_path(&self, room: &Room) -> PathBuf {
        self.root
            .join("world")
            .join(room.id.zone())
            .join("rooms")
            .join(format!("{}.ron", room.id.name()))
    }
}

impl WorldStore for GitStore {
    fn save_room(&mut self, room: &Room) -> Result<(), StoreError> {
        let path = self.room_path(room);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        fs::write(&path, room_to_ron(room)).map_err(io_err)?;
        Ok(())
    }

    fn load_all(&self) -> Result<World, StoreError> {
        let mut world = World::default();
        let world_dir = self.root.join("world");
        if !world_dir.exists() {
            return Ok(world);
        }
        for entry in WalkDir::new(&world_dir).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("ron") {
                continue;
            }
            let text = fs::read_to_string(path).map_err(io_err)?;
            let room = room_from_ron(&text).map_err(|e| StoreError::Parse(e.to_string()))?;
            world.insert_room(room);
        }
        Ok(world)
    }

    fn commit(&mut self, message: &str, epoch_seconds: i64) -> Result<CommitId, StoreError> {
        let mut index = self.repo.index().map_err(git_err)?;
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .map_err(git_err)?;
        index.write().map_err(git_err)?;
        let tree_oid = index.write_tree().map_err(git_err)?;
        let tree = self.repo.find_tree(tree_oid).map_err(git_err)?;

        let time = Time::new(epoch_seconds, 0);
        let sig = Signature::new("Snakewood", "world@snakewood.local", &time).map_err(git_err)?;

        let parent = match self.repo.head() {
            Ok(head) => Some(head.peel_to_commit().map_err(git_err)?),
            Err(_) => None,
        };
        let parents: Vec<&git2::Commit> = parent.iter().collect();

        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .map_err(git_err)?;
        Ok(CommitId(oid.to_string()))
    }

    fn commit_log(&self) -> Vec<String> {
        let mut messages = Vec::new();
        if let Ok(mut revwalk) = self.repo.revwalk() {
            if revwalk.push_head().is_ok() {
                for oid in revwalk.flatten() {
                    if let Ok(commit) = self.repo.find_commit(oid) {
                        messages.push(commit.message().unwrap_or("").to_string());
                    }
                }
            }
        }
        messages.reverse(); // oldest first
        messages
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use super::*;
    use crate::{Direction, EntityId};

    fn clearing() -> Room {
        let mut exits = BTreeMap::new();
        exits.insert(Direction::North, EntityId::new("snakewood/old-well").unwrap());
        Room {
            id: EntityId::new("snakewood/clearing").unwrap(),
            name: "Snakewood Clearing".to_string(),
            description: "A clearing.".to_string(),
            exits,
        }
    }

    fn old_well() -> Room {
        Room {
            id: EntityId::new("snakewood/old-well").unwrap(),
            name: "The Old Well".to_string(),
            description: "A crumbling stone well.".to_string(),
            exits: BTreeMap::new(),
        }
    }

    #[test]
    fn round_trips_through_git_and_reload() {
        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();
        store.save_room(&clearing()).unwrap();
        store.save_room(&old_well()).unwrap();
        store.commit("dig snakewood clearing and old well", 1_700_000_000).unwrap();

        // Fresh store over the same directory reloads an identical world.
        let reloaded = GitStore::init(dir.path()).unwrap().load_all().unwrap();
        let mut expected = World::default();
        expected.insert_room(clearing());
        expected.insert_room(old_well());
        assert_eq!(reloaded, expected);
    }

    #[test]
    fn commit_is_recorded_in_git_history() {
        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();
        store.save_room(&clearing()).unwrap();
        store.commit("dig snakewood clearing", 1_700_000_000).unwrap();
        assert_eq!(store.commit_log(), vec!["dig snakewood clearing".to_string()]);
    }

    #[test]
    fn writes_one_ron_file_per_room_at_expected_path() {
        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();
        store.save_room(&clearing()).unwrap();
        let expected = dir.path().join("world/snakewood/rooms/clearing.ron");
        assert!(expected.exists(), "expected room file at {expected:?}");
    }
}
```

- [ ] **Step 2: Export `GitStore` from `crates/snakewood-core/src/store/mod.rs`**

Add to `crates/snakewood-core/src/store/mod.rs`:

```rust
pub mod git;

pub use git::GitStore;
```

And add to `crates/snakewood-core/src/lib.rs` the re-export:

```rust
pub use store::GitStore;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-core store::git`
Expected: PASS (`round_trips_through_git_and_reload`, `commit_is_recorded_in_git_history`, `writes_one_ron_file_per_room_at_expected_path`).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/store/git.rs crates/snakewood-core/src/store/mod.rs crates/snakewood-core/src/lib.rs
git commit -m "feat: add git-backed WorldStore with one-file-per-room layout"
```

---

### Task 8: Round-trip property test and golden-file snapshot

**Files:**
- Create: `crates/snakewood-core/tests/roundtrip.rs`
- Create: `crates/snakewood-core/tests/golden/clearing.ron`

**Interfaces:**
- Consumes: `Room`, `World`, `Direction`, `EntityId`, `room_to_ron`, `GitStore`, `WorldStore` (public API, Tasks 2–7).
- Produces: an integration test crate proving (a) any generated world round-trips through the git store unchanged, and (b) a known room serializes byte-identically to a committed golden file.

- [ ] **Step 1: Write the golden file `crates/snakewood-core/tests/golden/clearing.ron`**

Generate the exact expected content first by running this throwaway snippet, then paste its output into the golden file:

```bash
cat > /tmp/gen_golden.rs <<'EOF'
// scratch: prints canonical RON for the golden room
fn main() {}
EOF
```

Instead of a scratch binary, produce the golden by writing the test in Step 3 with a temporary `println!` — but simplest: create the file with this content (canonical RON for the room the test builds; `PrettyConfig` uses 4-space indent and `struct_names(true)`):

```ron
Room(
    id: "snakewood/clearing",
    name: "Snakewood Clearing",
    description: "Gnarled snakewood trees ring a clearing.",
    exits: {
        North: "snakewood/old-well",
    },
)
```

> If the assertion in Step 4 fails on a formatting mismatch, do NOT hand-tweak the RON: instead run the test once with `TROUBLESHOOT=1` (see Step 3) to print the actual `room_to_ron` output, then overwrite the golden file with that exact output and re-run. The golden captures whatever canonical form the serializer produces; the test's job is to catch *unintended* drift.

- [ ] **Step 2: Write the property test in `crates/snakewood-core/tests/roundtrip.rs`**

```rust
use std::collections::BTreeMap;

use proptest::prelude::*;
use tempfile::tempdir;

use snakewood_core::{Direction, EntityId, GitStore, Room, World, WorldStore};

// Strategy for a valid id name segment: 1-8 chars from [a-z0-9-].
fn name_seg() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-z][a-z0-9-]{0,7}").unwrap()
}

fn arb_room(zone: &'static str) -> impl Strategy<Value = Room> {
    (
        name_seg(),
        ".*",
        ".*",
        prop::collection::btree_map(
            prop_oneof![
                Just(Direction::North),
                Just(Direction::South),
                Just(Direction::East),
                Just(Direction::West),
                Just(Direction::Up),
                Just(Direction::Down),
            ],
            name_seg(),
            0..6,
        ),
    )
        .prop_map(move |(name, desc, _extra, exit_names)| {
            let mut exits = BTreeMap::new();
            for (dir, target) in exit_names {
                exits.insert(dir, EntityId::new(format!("{zone}/{target}")).unwrap());
            }
            Room {
                id: EntityId::new(format!("{zone}/{name}")).unwrap(),
                name,
                description: desc,
                exits,
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_world_round_trips_through_git(rooms in prop::collection::vec(arb_room("snakewood"), 1..8)) {
        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();

        let mut expected = World::default();
        for room in &rooms {
            store.save_room(room).unwrap();
            expected.insert_room(room.clone());
        }
        store.commit("proptest world", 1_700_000_000).unwrap();

        let reloaded = GitStore::init(dir.path()).unwrap().load_all().unwrap();
        prop_assert_eq!(reloaded, expected);
    }
}
```

- [ ] **Step 3: Write the golden-file test in the same file `crates/snakewood-core/tests/roundtrip.rs`**

Append:

```rust
#[test]
fn known_room_matches_golden() {
    use snakewood_core::room_to_ron;

    let mut exits = BTreeMap::new();
    exits.insert(Direction::North, EntityId::new("snakewood/old-well").unwrap());
    let room = Room {
        id: EntityId::new("snakewood/clearing").unwrap(),
        name: "Snakewood Clearing".to_string(),
        description: "Gnarled snakewood trees ring a clearing.".to_string(),
        exits,
    };

    let actual = room_to_ron(&room);
    if std::env::var("TROUBLESHOOT").is_ok() {
        eprintln!("--- actual room_to_ron output ---\n{actual}\n--- end ---");
    }
    let golden = include_str!("golden/clearing.ron");
    assert_eq!(actual.trim(), golden.trim(), "serialized room drifted from golden file");
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p snakewood-core --test roundtrip`
Expected: PASS (`any_world_round_trips_through_git`, `known_room_matches_golden`). If `known_room_matches_golden` fails on formatting, run `TROUBLESHOOT=1 cargo test -p snakewood-core --test roundtrip known_room_matches_golden -- --nocapture`, copy the printed output into `tests/golden/clearing.ron`, and re-run.

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-core/tests/roundtrip.rs crates/snakewood-core/tests/golden/clearing.ron
git commit -m "test: prove world round-trips through git via proptest and golden file"
```

---

### Task 9: Stage-completion verification

**Files:** none (verification only).

**Interfaces:**
- Consumes: the entire crate.
- Produces: confidence that Stage 1 is green and clean.

- [ ] **Step 1: Run the full test suite**

Run: `cargo test -p snakewood-core`
Expected: all tests pass (smoke, direction, id, world, serialize, store::memory, store::git, roundtrip integration).

- [ ] **Step 2: Run clippy and formatting checks**

Run: `cargo clippy -p snakewood-core --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no formatting diffs. (If `cargo fmt --check` reports diffs, run `cargo fmt` and commit the result.)

- [ ] **Step 3: Confirm the git-backed store produced real history in a scratch run (optional sanity)**

This is covered by `commit_is_recorded_in_git_history`; no extra action needed. Stage 1 is complete when Steps 1–2 are green.

---

## Self-Review

**1. Spec coverage (Stage 1 slice of M1 §7):**
- Workspace `snakewood-core` — Task 1. ✓
- Injected time (Clock deferred to Stage 2; commit timestamp injected as a parameter now) — Tasks 6–7. ✓
- `Room` with sugar exits, hand-authored rooms in RON — Tasks 4, 8. ✓
- Canonical deterministic serializer — Task 5. ✓
- `WorldStore` trait with git-backed impl + temp/in-memory impl — Tasks 6, 7. ✓
- Round-trip proof (proptest + golden file) — Task 8. ✓
- Deferred to Stage 2/3 (correctly absent here): fabric, Intent/Event, presentation, transports, MCP, goblin, persistence scheduler/policies, tick loop. ✓

**2. Placeholder scan:** No "TBD/TODO/implement later". The golden-file value is concrete, with an explicit regeneration procedure if `PrettyConfig` output differs from the hand-written expectation (this is a legitimate golden-capture step, not a placeholder). ✓

**3. Type consistency:** `EntityId::new` returns `Result<_, IdError>` (Task 3) and is `.unwrap()`ed in test fixtures consistently. `WorldStore` methods (`save_room`, `load_all`, `commit(&mut self, &str, i64)`, `commit_log`) match between the trait (Task 6) and both impls (Tasks 6, 7) and the property test (Task 8). `room_to_ron`/`room_from_ron` names match across Tasks 5, 7, 8. `Direction` variant order (North, South, East, West, Up, Down) underpins the "Down before North" sort assertion in Task 5 and the golden file in Task 8. ✓
