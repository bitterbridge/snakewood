# Snakewood M1 Stage 2 — Event Fabric Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the core event fabric — Intent → Guard → Commit → Notify — as a pure, deterministic library on top of Stage 1's world model, proven by an end-to-end "blocking goblin" scenario where a predicate-gated mob denies movement and its removal opens the path.

**Architecture:** All new logic lives in `snakewood-core` (still pure — no async, networking, or persistence; those are Stage 3). Stage 1's `World` (authored rooms) is left untouched; a new `Realm` type wraps `World` plus live `mobs` and global `rules`. A single `dispatch(&mut Realm, Intent) -> Dispatch` entry point runs the three phases: **Guard** (two passes — resolve the outcome set-based via the `Deny > Allow` lattice, then select narration/reaction by salience), **Commit** (apply the state change), and **Notify** (publish events; observer-driven reactions are stubbed and deferred to Stage 3). Handlers are data (`Responder` on mobs, global `Rule`s) built from tiny fixed vocabularies (`Trigger`, `Predicate`, `Effect`, `Outcome`). The core emits a semantic `PresentationNode` stream, never formatted text.

**Tech Stack:** Rust (edition 2021), `serde` (derive) on persisted data types, existing `snakewood-core` crate. No new dependencies.

## Global Constraints

- **Language:** Rust, edition 2021. All work is in the existing `snakewood-core` crate; no new crates, no new dependencies.
- **Pure and deterministic:** no async, no threads, no I/O, no wall-clock, no RNG in Stage 2. `dispatch` is a deterministic function of `(Realm, Intent)`.
- **No embedded scripting.** Handlers are data composed from fixed enum vocabularies; a fixed Rust evaluator interprets them.
- **`World` is FROZEN.** Do not modify `crates/snakewood-core/src/world.rs` or its serialization. All new state goes on the new `Realm` type. This keeps Stage 1's round-trip tests green.
- **Determinism of dispatch:** the **outcome** is set-based (order-independent): the lattice is **`Deny` (Block) > `Allow`/`Traverse`**; any `Block` denies; an `Allow`/`Traverse` never overrides a `Block`. Only **narration/reaction selection** is ordered, by **salience**: band order **Participant → Structure → Global** (Participant most salient), tie-broken by higher **priority** integer, then by stable `EntityId` order.
- **Derived + guarded subscription:** never store subscription lists. Candidate handlers are computed each dispatch from co-presence (mobs in the actor's room) + the room's sugar exits + global rules; participation is gated by `require` predicates evaluated against current state.
- **Collections:** use `BTreeMap`/`BTreeSet` (deterministic ordering), never `HashMap`/`HashSet`.
- **Serialization:** derive `serde::{Serialize, Deserialize}` on data types destined for RON persistence (`Mob`, `Flag`, `Responder`, `Rule`, `Trigger`, `Predicate`, `Party`, `Effect`, `Outcome`). Runtime-only types (`Intent`, `Event`, `PresentationNode`, `Dispatch`, `Candidate`) derive `Debug, Clone, PartialEq` but not serde.
- **Deferred to later stages (do NOT build):** Redirect outcomes, room-attached responders, locked doors, the tick loop, operators, observer-emitted intents (Notify reactions), combat, prototypes, item entities, persistence of mobs/rules.

---

### Task 1: `Flag` and `Mob` (live located creatures)

**Files:**
- Create: `crates/snakewood-core/src/mob.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: `crate::EntityId`.
- Produces:
  - `pub enum Flag { Alive, Conscious }` deriving `Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`.
  - `pub struct Mob { pub id: EntityId, pub name: String, pub location: EntityId, pub flags: BTreeSet<Flag>, pub responders: Vec<Responder> }` deriving `Serialize, Deserialize, Debug, Clone, PartialEq`. NOTE: `responders` is typed `Vec<crate::fabric::Responder>`, which does not exist until Task 6 — so this task defines `Mob` WITHOUT the `responders` field, and Task 6 adds it. (See Step 1.)
  - `Mob::has_flag(&self, flag: Flag) -> bool`.

- [ ] **Step 1: Write `crates/snakewood-core/src/mob.rs` with its tests**

```rust
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::EntityId;

/// A boolean state marker on a mob. Guards (predicates) test these.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Flag {
    Alive,
    Conscious,
}

/// A live, located creature (both player-characters and NPCs, in Stage 2).
/// `responders` (data handlers) are added in Task 6 once `Responder` exists.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Mob {
    pub id: EntityId,
    pub name: String,
    /// The id of the room this mob currently occupies.
    pub location: EntityId,
    #[serde(default)]
    pub flags: BTreeSet<Flag>,
}

impl Mob {
    pub fn has_flag(&self, flag: Flag) -> bool {
        self.flags.contains(&flag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goblin() -> Mob {
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        flags.insert(Flag::Conscious);
        Mob {
            id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
            name: "a snakewood goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
        }
    }

    #[test]
    fn has_flag_reflects_membership() {
        let g = goblin();
        assert!(g.has_flag(Flag::Alive));
        assert!(g.has_flag(Flag::Conscious));
    }

    #[test]
    fn missing_flag_is_false() {
        let mut g = goblin();
        g.flags.remove(&Flag::Alive);
        assert!(!g.has_flag(Flag::Alive));
    }
}
```

- [ ] **Step 2: Wire the module into `lib.rs`**

Add (keeping existing lines, alphabetical among modules):

```rust
pub mod mob;
```
and in the re-export block:
```rust
pub use mob::{Flag, Mob};
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-core mob`
Expected: PASS (`has_flag_reflects_membership`, `missing_flag_is_false`).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/mob.rs crates/snakewood-core/src/lib.rs
git commit -m "feat: add Mob and Flag (live located creatures)"
```

---

### Task 2: `Realm` (authored world + live mobs + rules) with derived co-presence

**Files:**
- Create: `crates/snakewood-core/src/realm.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: `crate::{World, EntityId, Mob}`.
- Produces:
  - `pub struct Realm { pub world: World, pub mobs: BTreeMap<EntityId, Mob>, pub rules: Vec<Rule> }` deriving `Debug, Clone, Default`. NOTE: `rules: Vec<Rule>` references `crate::fabric::Rule` (Task 6); this task defines `Realm` WITHOUT the `rules` field, and Task 6 adds it.
  - `Realm::new(world: World) -> Realm`.
  - `Realm::insert_mob(&mut self, mob: Mob)` — keyed by `mob.id`.
  - `Realm::mob(&self, id: &EntityId) -> Option<&Mob>`.
  - `Realm::mob_mut(&mut self, id: &EntityId) -> Option<&mut Mob>`.
  - `Realm::mob_location(&self, id: &EntityId) -> Option<&EntityId>` — the room a mob occupies.
  - `Realm::mobs_in_room(&self, room: &EntityId) -> Vec<&Mob>` — DERIVED co-presence, sorted by `EntityId` for determinism; computed each call, never cached.

- [ ] **Step 1: Write `crates/snakewood-core/src/realm.rs` with its tests**

```rust
use std::collections::BTreeMap;

use crate::{EntityId, Mob, World};

/// The fabric's operating context: authored rooms (`world`) plus live `mobs`.
/// Global `rules` are added in Task 6. Co-presence is derived on demand.
#[derive(Debug, Clone, Default)]
pub struct Realm {
    pub world: World,
    pub mobs: BTreeMap<EntityId, Mob>,
}

impl Realm {
    pub fn new(world: World) -> Realm {
        Realm {
            world,
            mobs: BTreeMap::new(),
        }
    }

    pub fn insert_mob(&mut self, mob: Mob) {
        self.mobs.insert(mob.id.clone(), mob);
    }

    pub fn mob(&self, id: &EntityId) -> Option<&Mob> {
        self.mobs.get(id)
    }

    pub fn mob_mut(&mut self, id: &EntityId) -> Option<&mut Mob> {
        self.mobs.get_mut(id)
    }

    pub fn mob_location(&self, id: &EntityId) -> Option<&EntityId> {
        self.mobs.get(id).map(|m| &m.location)
    }

    /// All mobs currently in `room`, sorted by id (deterministic). Derived each
    /// call — there is no stored subscription list to go stale.
    pub fn mobs_in_room(&self, room: &EntityId) -> Vec<&Mob> {
        self.mobs.values().filter(|m| &m.location == room).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{Flag, World};

    fn mob_at(id: &str, room: &str) -> Mob {
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        Mob {
            id: EntityId::new(id).unwrap(),
            name: id.to_string(),
            location: EntityId::new(room).unwrap(),
            flags,
        }
    }

    #[test]
    fn mobs_in_room_returns_only_co_located_sorted() {
        let mut realm = Realm::new(World::default());
        realm.insert_mob(mob_at("snakewood/mob/b#1", "snakewood/clearing"));
        realm.insert_mob(mob_at("snakewood/mob/a#1", "snakewood/clearing"));
        realm.insert_mob(mob_at("snakewood/mob/c#1", "snakewood/old-well"));

        let clearing = EntityId::new("snakewood/clearing").unwrap();
        let here: Vec<&str> = realm.mobs_in_room(&clearing).iter().map(|m| m.id.as_str()).collect();
        // BTreeMap iteration is sorted, so a before b; c is elsewhere.
        assert_eq!(here, vec!["snakewood/mob/a#1", "snakewood/mob/b#1"]);
    }

    #[test]
    fn mob_location_tracks_current_room() {
        let mut realm = Realm::new(World::default());
        realm.insert_mob(mob_at("snakewood/mob/a#1", "snakewood/clearing"));
        let a = EntityId::new("snakewood/mob/a#1").unwrap();
        assert_eq!(realm.mob_location(&a).map(|r| r.as_str()), Some("snakewood/clearing"));
    }
}
```

- [ ] **Step 2: Wire the module into `lib.rs`**

Add:
```rust
pub mod realm;
```
and:
```rust
pub use realm::Realm;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-core realm`
Expected: PASS (`mobs_in_room_returns_only_co_located_sorted`, `mob_location_tracks_current_room`).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/realm.rs crates/snakewood-core/src/lib.rs
git commit -m "feat: add Realm with derived co-presence"
```

---

### Task 3: `Intent`, `Event`, and the `fabric` module

**Files:**
- Create: `crates/snakewood-core/src/fabric/mod.rs`
- Create: `crates/snakewood-core/src/fabric/intent.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: `crate::{Direction, EntityId}`.
- Produces:
  - `pub enum Intent { Move { actor: EntityId, direction: Direction }, Look { actor: EntityId } }` deriving `Debug, Clone, PartialEq`.
  - `pub enum Event { Moved { actor: EntityId, from: EntityId, to: EntityId }, Looked { actor: EntityId, room: EntityId } }` deriving `Debug, Clone, PartialEq`.
  - `Intent::actor(&self) -> &EntityId`.
  - `crate::fabric` module, re-exported at crate root as needed.

- [ ] **Step 1: Write `crates/snakewood-core/src/fabric/intent.rs` with tests**

```rust
use crate::{Direction, EntityId};

/// A proposed, vetoable action entering the fabric.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Move { actor: EntityId, direction: Direction },
    Look { actor: EntityId },
}

/// A committed, factual, observable occurrence produced by the Commit phase.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Moved { actor: EntityId, from: EntityId, to: EntityId },
    Looked { actor: EntityId, room: EntityId },
}

impl Intent {
    /// The entity performing the intent.
    pub fn actor(&self) -> &EntityId {
        match self {
            Intent::Move { actor, .. } => actor,
            Intent::Look { actor } => actor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_accessor_returns_the_actor() {
        let a = EntityId::new("snakewood/pc/nathan").unwrap();
        let intent = Intent::Move { actor: a.clone(), direction: Direction::North };
        assert_eq!(intent.actor(), &a);
        let look = Intent::Look { actor: a.clone() };
        assert_eq!(look.actor(), &a);
    }
}
```

- [ ] **Step 2: Write `crates/snakewood-core/src/fabric/mod.rs`**

```rust
//! The event fabric: Intent -> Guard -> Commit -> Notify.

pub mod intent;

pub use intent::{Event, Intent};
```

- [ ] **Step 3: Wire the module into `lib.rs`**

Add:
```rust
pub mod fabric;
```
and:
```rust
pub use fabric::{Event, Intent};
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p snakewood-core fabric::intent`
Expected: PASS (`actor_accessor_returns_the_actor`).

- [ ] **Step 5: Commit**

```bash
git add crates/snakewood-core/src/fabric crates/snakewood-core/src/lib.rs
git commit -m "feat: add Intent and Event types and fabric module"
```

---

### Task 4: `Trigger` (intent-pattern matching)

**Files:**
- Create: `crates/snakewood-core/src/fabric/trigger.rs`
- Modify: `crates/snakewood-core/src/fabric/mod.rs`

**Interfaces:**
- Consumes: `crate::{Direction}`, `crate::fabric::Intent`.
- Produces:
  - `pub enum Trigger { Move(Direction), AnyMove, Look }` deriving `Serialize, Deserialize, Debug, Clone, PartialEq`.
  - `Trigger::matches(&self, intent: &Intent) -> bool`.

- [ ] **Step 1: Write `crates/snakewood-core/src/fabric/trigger.rs` with tests**

```rust
use serde::{Deserialize, Serialize};

use crate::fabric::Intent;
use crate::Direction;

/// A pattern a handler listens for. `AnyMove` matches movement in any direction.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Trigger {
    Move(Direction),
    AnyMove,
    Look,
}

impl Trigger {
    pub fn matches(&self, intent: &Intent) -> bool {
        match (self, intent) {
            (Trigger::Move(d), Intent::Move { direction, .. }) => d == direction,
            (Trigger::AnyMove, Intent::Move { .. }) => true,
            (Trigger::Look, Intent::Look { .. }) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntityId;

    fn move_north() -> Intent {
        Intent::Move {
            actor: EntityId::new("snakewood/pc/nathan").unwrap(),
            direction: Direction::North,
        }
    }

    #[test]
    fn exact_direction_matches_only_that_direction() {
        assert!(Trigger::Move(Direction::North).matches(&move_north()));
        assert!(!Trigger::Move(Direction::South).matches(&move_north()));
    }

    #[test]
    fn any_move_matches_all_moves_but_not_look() {
        let look = Intent::Look { actor: EntityId::new("snakewood/pc/nathan").unwrap() };
        assert!(Trigger::AnyMove.matches(&move_north()));
        assert!(!Trigger::AnyMove.matches(&look));
    }

    #[test]
    fn look_trigger_matches_look() {
        let look = Intent::Look { actor: EntityId::new("snakewood/pc/nathan").unwrap() };
        assert!(Trigger::Look.matches(&look));
        assert!(!Trigger::Look.matches(&move_north()));
    }
}
```

- [ ] **Step 2: Wire into `crates/snakewood-core/src/fabric/mod.rs`**

Add:
```rust
pub mod trigger;
```
and to its re-export line:
```rust
pub use trigger::Trigger;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-core fabric::trigger`
Expected: PASS (3 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/fabric/trigger.rs crates/snakewood-core/src/fabric/mod.rs
git commit -m "feat: add Trigger intent-pattern matching"
```

---

### Task 5: `Party`, `Predicate`, and predicate evaluation

**Files:**
- Create: `crates/snakewood-core/src/fabric/predicate.rs`
- Modify: `crates/snakewood-core/src/fabric/mod.rs`

**Interfaces:**
- Consumes: `crate::{Realm, EntityId, Flag}`.
- Produces:
  - `pub enum Party { Actor, SelfMob }` deriving `Serialize, Deserialize, Debug, Clone, Copy, PartialEq`.
  - `pub enum Predicate { Alive(Party), Conscious(Party) }` deriving `Serialize, Deserialize, Debug, Clone, PartialEq`.
  - `pub fn eval(realm: &Realm, pred: &Predicate, self_id: Option<&EntityId>, actor: &EntityId) -> bool` — resolves `Party` to a concrete mob (`Actor` → `actor`; `SelfMob` → `self_id`), then checks the flag. A `Party` that cannot be resolved (e.g. `SelfMob` with `self_id == None`, or a mob that does not exist) evaluates to `false`.

- [ ] **Step 1: Write `crates/snakewood-core/src/fabric/predicate.rs` with tests**

```rust
use serde::{Deserialize, Serialize};

use crate::{EntityId, Flag, Realm};

/// Which participant a predicate/effect refers to. `SelfMob` is the mob that
/// owns the responder (None for global rules).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum Party {
    Actor,
    SelfMob,
}

/// A guard, drawn from the fixed predicate vocabulary.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Predicate {
    Alive(Party),
    Conscious(Party),
}

fn resolve<'a>(party: Party, self_id: Option<&'a EntityId>, actor: &'a EntityId) -> Option<&'a EntityId> {
    match party {
        Party::Actor => Some(actor),
        Party::SelfMob => self_id,
    }
}

fn mob_has(realm: &Realm, who: Option<&EntityId>, flag: Flag) -> bool {
    match who.and_then(|id| realm.mob(id)) {
        Some(mob) => mob.has_flag(flag),
        None => false,
    }
}

/// Evaluate a predicate against current state. Unresolvable parties → false.
pub fn eval(realm: &Realm, pred: &Predicate, self_id: Option<&EntityId>, actor: &EntityId) -> bool {
    match pred {
        Predicate::Alive(p) => mob_has(realm, resolve(*p, self_id, actor), Flag::Alive),
        Predicate::Conscious(p) => mob_has(realm, resolve(*p, self_id, actor), Flag::Conscious),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{Mob, World};

    fn realm_with_goblin(alive: bool, conscious: bool) -> (Realm, EntityId, EntityId) {
        let mut flags = BTreeSet::new();
        if alive {
            flags.insert(Flag::Alive);
        }
        if conscious {
            flags.insert(Flag::Conscious);
        }
        let goblin_id = EntityId::new("snakewood/mob/goblin#1").unwrap();
        let actor_id = EntityId::new("snakewood/pc/nathan").unwrap();
        let mut realm = Realm::new(World::default());
        realm.insert_mob(Mob {
            id: goblin_id.clone(),
            name: "goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
        });
        (realm, goblin_id, actor_id)
    }

    #[test]
    fn self_alive_true_when_alive() {
        let (realm, goblin, actor) = realm_with_goblin(true, true);
        assert!(eval(&realm, &Predicate::Alive(Party::SelfMob), Some(&goblin), &actor));
    }

    #[test]
    fn self_alive_false_when_dead() {
        let (realm, goblin, actor) = realm_with_goblin(false, true);
        assert!(!eval(&realm, &Predicate::Alive(Party::SelfMob), Some(&goblin), &actor));
    }

    #[test]
    fn self_predicate_false_when_no_self_id() {
        let (realm, _goblin, actor) = realm_with_goblin(true, true);
        assert!(!eval(&realm, &Predicate::Alive(Party::SelfMob), None, &actor));
    }

    #[test]
    fn conscious_reflects_flag() {
        let (realm, goblin, actor) = realm_with_goblin(true, false);
        assert!(!eval(&realm, &Predicate::Conscious(Party::SelfMob), Some(&goblin), &actor));
    }
}
```

- [ ] **Step 2: Wire into `crates/snakewood-core/src/fabric/mod.rs`**

Add:
```rust
pub mod predicate;
```
and:
```rust
pub use predicate::{eval as eval_predicate, Party, Predicate};
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-core fabric::predicate`
Expected: PASS (4 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/fabric/predicate.rs crates/snakewood-core/src/fabric/mod.rs
git commit -m "feat: add Party, Predicate, and predicate evaluation"
```

---

### Task 6: `PresentationNode`, `Effect`, `Outcome`, `Band`, `Responder`, `Rule`; add `responders`/`rules` fields

**Files:**
- Create: `crates/snakewood-core/src/presentation.rs`
- Create: `crates/snakewood-core/src/fabric/handler.rs`
- Modify: `crates/snakewood-core/src/fabric/mod.rs`
- Modify: `crates/snakewood-core/src/mob.rs` (add `responders` field)
- Modify: `crates/snakewood-core/src/realm.rs` (add `rules` field)
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: `crate::{Direction, EntityId}`, `crate::fabric::{Trigger, Predicate, Party}`.
- Produces:
  - `pub enum PresentationNode { RoomName(String), RoomDescription(String), Exits(Vec<Direction>), Occupants(Vec<String>), Line(String), Denied(String), Prompt }` deriving `Debug, Clone, PartialEq`.
  - `pub enum Effect { Narrate(Party, String) }` deriving `Serialize, Deserialize, Debug, Clone, PartialEq`.
  - `pub enum Outcome { Traverse(EntityId), Block, Allow }` deriving `Serialize, Deserialize, Debug, Clone, PartialEq`.
  - `pub enum Band { Participant, Structure, Global }` deriving `Debug, Clone, Copy, PartialEq, Eq`. `Band::rank(&self) -> u8` returns `0/1/2` (Participant most salient).
  - `pub struct Responder { pub on: Trigger, #[serde(default)] pub require: Vec<Predicate>, #[serde(default)] pub effects: Vec<Effect>, pub outcome: Outcome, #[serde(default)] pub priority: i32 }` deriving `Serialize, Deserialize, Debug, Clone, PartialEq`.
  - `pub struct Rule { pub on: Trigger, #[serde(default)] pub require: Vec<Predicate>, #[serde(default)] pub effects: Vec<Effect>, pub outcome: Outcome, #[serde(default)] pub priority: i32 }` deriving `Serialize, Deserialize, Debug, Clone, PartialEq`.
  - `Mob` gains `#[serde(default)] pub responders: Vec<Responder>`.
  - `Realm` gains `#[serde(skip)]`-free `pub rules: Vec<Rule>` (plain field; `Realm` is not serialized in Stage 2).

- [ ] **Step 1: Write `crates/snakewood-core/src/presentation.rs`**

```rust
use crate::Direction;

/// A semantic unit of output. Transports (Stage 3) render these; the core never
/// emits formatted text or ANSI.
#[derive(Debug, Clone, PartialEq)]
pub enum PresentationNode {
    RoomName(String),
    RoomDescription(String),
    Exits(Vec<Direction>),
    Occupants(Vec<String>),
    Line(String),
    Denied(String),
    Prompt,
}
```

- [ ] **Step 2: Write `crates/snakewood-core/src/fabric/handler.rs` with tests**

```rust
use serde::{Deserialize, Serialize};

use crate::fabric::{Party, Predicate, Trigger};
use crate::EntityId;

/// An effect a handler runs when it is the salient responder.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Effect {
    Narrate(Party, String),
}

/// A handler's vote/result. `Traverse` allows movement to a room; `Block` denies;
/// `Allow` is a generic non-movement allow. (Redirect is deferred to a later stage.)
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Outcome {
    Traverse(EntityId),
    Block,
    Allow,
}

/// Salience band. Lower rank = more salient (narrates first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Participant,
    Structure,
    Global,
}

impl Band {
    pub fn rank(&self) -> u8 {
        match self {
            Band::Participant => 0,
            Band::Structure => 1,
            Band::Global => 2,
        }
    }
}

/// An entity-attached handler (lives on a `Mob`, Participant band).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Responder {
    pub on: Trigger,
    #[serde(default)]
    pub require: Vec<Predicate>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    pub outcome: Outcome,
    #[serde(default)]
    pub priority: i32,
}

/// A global, unattached handler (Global band).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Rule {
    pub on: Trigger,
    #[serde(default)]
    pub require: Vec<Predicate>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    pub outcome: Outcome,
    #[serde(default)]
    pub priority: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_rank_orders_participant_most_salient() {
        assert!(Band::Participant.rank() < Band::Structure.rank());
        assert!(Band::Structure.rank() < Band::Global.rank());
    }
}
```

- [ ] **Step 3: Add `responders` to `Mob`**

In `crates/snakewood-core/src/mob.rs`, add the import and field. Change the top `use` and the struct:

```rust
use crate::fabric::Responder;
use crate::EntityId;
```
and add to the `Mob` struct (after `flags`):
```rust
    #[serde(default)]
    pub responders: Vec<Responder>,
```
Update the `mob.rs` test fixtures (`goblin()` and any `Mob { .. }` literal in that file's tests) to include `responders: Vec::new(),`.

- [ ] **Step 4: Add `rules` to `Realm`**

In `crates/snakewood-core/src/realm.rs`:
- Add import: `use crate::fabric::Rule;`
- Add field to the struct: `pub rules: Vec<Rule>,`
- In `Realm::new`, initialize `rules: Vec::new(),`.
- Update the `realm.rs` test `mob_at` fixture `Mob { .. }` literal to include `responders: Vec::new(),`.

- [ ] **Step 5: Wire modules into `mod.rs` and `lib.rs`**

In `crates/snakewood-core/src/fabric/mod.rs` add:
```rust
pub mod handler;
```
and:
```rust
pub use handler::{Band, Effect, Outcome, Responder, Rule};
```

In `crates/snakewood-core/src/lib.rs` add:
```rust
pub mod presentation;
```
and:
```rust
pub use fabric::{Band, Effect, Outcome, Responder, Rule};
pub use presentation::PresentationNode;
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p snakewood-core`
Expected: all pass (including `fabric::handler::tests::band_rank_orders_participant_most_salient` and the updated `mob`/`realm` tests, plus all Stage 1 tests still green).

- [ ] **Step 7: Commit**

```bash
git add crates/snakewood-core/src/presentation.rs crates/snakewood-core/src/fabric/handler.rs crates/snakewood-core/src/fabric/mod.rs crates/snakewood-core/src/mob.rs crates/snakewood-core/src/realm.rs crates/snakewood-core/src/lib.rs
git commit -m "feat: add presentation nodes, effects, outcomes, responders, and rules"
```

---

### Task 7: Gather candidates (derived + guarded subscription)

**Files:**
- Create: `crates/snakewood-core/src/fabric/gather.rs`
- Modify: `crates/snakewood-core/src/fabric/mod.rs`

**Interfaces:**
- Consumes: `crate::{Realm, EntityId}`, `crate::fabric::{Intent, Trigger, Predicate, eval_predicate, Band, Effect, Outcome}`, `crate::world::Room`.
- Produces:
  - `pub struct Candidate { pub band: Band, pub priority: i32, pub self_id: Option<EntityId>, pub outcome: Outcome, pub effects: Vec<Effect> }` deriving `Debug, Clone, PartialEq`.
  - `pub fn gather(realm: &Realm, intent: &Intent) -> Vec<Candidate>` — collects candidate handlers whose trigger matches and whose `require` predicates all pass, in this source order: sugar exits (Structure) → co-present mob responders excluding the actor (Participant) → global rules (Global). Sugar exits apply only to `Move`.

- [ ] **Step 1: Write `crates/snakewood-core/src/fabric/gather.rs` with tests**

```rust
use crate::fabric::{eval_predicate, Band, Effect, Intent, Outcome};
use crate::{EntityId, Realm};

/// A resolved handler contribution for one dispatch, after predicate filtering.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub band: Band,
    pub priority: i32,
    pub self_id: Option<EntityId>,
    pub outcome: Outcome,
    pub effects: Vec<Effect>,
}

fn require_passes(
    realm: &Realm,
    require: &[crate::fabric::Predicate],
    self_id: Option<&EntityId>,
    actor: &EntityId,
) -> bool {
    require.iter().all(|p| eval_predicate(realm, p, self_id, actor))
}

/// Compute candidate handlers for `intent` from current state. Never cached.
pub fn gather(realm: &Realm, intent: &Intent) -> Vec<Candidate> {
    let actor = intent.actor();
    let mut candidates = Vec::new();

    let Some(room_id) = realm.mob_location(actor).cloned() else {
        return candidates; // actor not located; nothing to gather
    };

    // Structure band: sugar exits (Move only).
    if let Intent::Move { direction, .. } = intent {
        if let Some(room) = realm.world.room(&room_id) {
            if let Some(dest) = room.exits.get(direction) {
                candidates.push(Candidate {
                    band: Band::Structure,
                    priority: 0,
                    self_id: None,
                    outcome: Outcome::Traverse(dest.clone()),
                    effects: Vec::new(),
                });
            }
        }
    }

    // Participant band: co-present mobs (excluding the actor).
    for mob in realm.mobs_in_room(&room_id) {
        if &mob.id == actor {
            continue;
        }
        for responder in &mob.responders {
            if responder.on.matches(intent)
                && require_passes(realm, &responder.require, Some(&mob.id), actor)
            {
                candidates.push(Candidate {
                    band: Band::Participant,
                    priority: responder.priority,
                    self_id: Some(mob.id.clone()),
                    outcome: responder.outcome.clone(),
                    effects: responder.effects.clone(),
                });
            }
        }
    }

    // Global band: rules.
    for rule in &realm.rules {
        if rule.on.matches(intent) && require_passes(realm, &rule.require, None, actor) {
            candidates.push(Candidate {
                band: Band::Global,
                priority: rule.priority,
                self_id: None,
                outcome: rule.outcome.clone(),
                effects: rule.effects.clone(),
            });
        }
    }

    candidates
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::fabric::{Party, Predicate, Responder, Trigger};
    use crate::{Direction, Flag, Mob, Room, World};

    fn world_two_rooms() -> World {
        let mut exits = BTreeMap::new();
        exits.insert(Direction::North, EntityId::new("snakewood/old-well").unwrap());
        let mut world = World::default();
        world.insert_room(Room {
            id: EntityId::new("snakewood/clearing").unwrap(),
            name: "Clearing".to_string(),
            description: "A clearing.".to_string(),
            exits,
        });
        world.insert_room(Room {
            id: EntityId::new("snakewood/old-well").unwrap(),
            name: "Old Well".to_string(),
            description: "A well.".to_string(),
            exits: BTreeMap::new(),
        });
        world
    }

    fn actor_id() -> EntityId {
        EntityId::new("snakewood/pc/nathan").unwrap()
    }

    fn place_actor(realm: &mut Realm) {
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        realm.insert_mob(Mob {
            id: actor_id(),
            name: "Nathan".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        });
    }

    fn blocking_goblin(alive: bool) -> Mob {
        let mut flags = BTreeSet::new();
        if alive {
            flags.insert(Flag::Alive);
            flags.insert(Flag::Conscious);
        }
        Mob {
            id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
            name: "a goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
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

    #[test]
    fn gathers_sugar_exit_as_structure_traverse() {
        let mut realm = Realm::new(world_two_rooms());
        place_actor(&mut realm);
        let intent = Intent::Move { actor: actor_id(), direction: Direction::North };
        let got = gather(&realm, &intent);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].band, Band::Structure);
        assert_eq!(got[0].outcome, Outcome::Traverse(EntityId::new("snakewood/old-well").unwrap()));
    }

    #[test]
    fn live_goblin_contributes_a_participant_block() {
        let mut realm = Realm::new(world_two_rooms());
        place_actor(&mut realm);
        realm.insert_mob(blocking_goblin(true));
        let intent = Intent::Move { actor: actor_id(), direction: Direction::North };
        let got = gather(&realm, &intent);
        // sugar exit (Structure) + goblin block (Participant)
        assert_eq!(got.len(), 2);
        assert!(got.iter().any(|c| c.band == Band::Participant && c.outcome == Outcome::Block));
    }

    #[test]
    fn dead_goblin_contributes_nothing_guarded_out() {
        let mut realm = Realm::new(world_two_rooms());
        place_actor(&mut realm);
        realm.insert_mob(blocking_goblin(false)); // no Alive/Conscious flags
        let intent = Intent::Move { actor: actor_id(), direction: Direction::North };
        let got = gather(&realm, &intent);
        // only the sugar exit; the goblin's require predicates fail
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].band, Band::Structure);
    }
}
```

- [ ] **Step 2: Wire into `crates/snakewood-core/src/fabric/mod.rs`**

Add:
```rust
pub mod gather;
```
and:
```rust
pub use gather::{gather, Candidate};
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-core fabric::gather`
Expected: PASS (3 tests), especially `dead_goblin_contributes_nothing_guarded_out` (proves derived + guarded subscription).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/fabric/gather.rs crates/snakewood-core/src/fabric/mod.rs
git commit -m "feat: gather candidate handlers with derived guarded subscription"
```

---

### Task 8: Resolve (Pass 1) and salience selection (Pass 2)

**Files:**
- Create: `crates/snakewood-core/src/fabric/resolve.rs`
- Modify: `crates/snakewood-core/src/fabric/mod.rs`

**Interfaces:**
- Consumes: `crate::fabric::{Candidate, Outcome, Band}`, `crate::EntityId`.
- Produces:
  - `pub enum Decision { Denied, Allowed { destination: EntityId }, Unresolved }` deriving `Debug, Clone, PartialEq`.
  - `pub fn resolve(candidates: &[Candidate]) -> Decision` — set-based lattice: any `Block` → `Denied`; else any `Traverse(d)` → `Allowed { destination }` (destination chosen by salience among traversers); else `Unresolved`.
  - `pub fn salient(candidates: &[Candidate]) -> Option<&Candidate>` — the most salient candidate among the given slice: lowest `band.rank()`, then highest `priority`, then stable by `self_id` (Some sorted by id; None last). Used by callers to pick the narrating/reacting candidate from a pre-filtered decisive subset.

- [ ] **Step 1: Write `crates/snakewood-core/src/fabric/resolve.rs` with tests**

```rust
use crate::fabric::{Candidate, Outcome};
use crate::EntityId;

/// The order-independent outcome of the Guard resolve pass.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Denied,
    Allowed { destination: EntityId },
    Unresolved,
}

/// Compare two candidates for salience. Returns true if `a` is MORE salient than `b`.
fn more_salient(a: &Candidate, b: &Candidate) -> bool {
    let (ra, rb) = (a.band.rank(), b.band.rank());
    if ra != rb {
        return ra < rb; // lower rank = more salient (Participant first)
    }
    if a.priority != b.priority {
        return a.priority > b.priority; // higher priority wins
    }
    // stable tie-break by self_id: Some(smaller id) beats larger; Some beats None
    match (&a.self_id, &b.self_id) {
        (Some(x), Some(y)) => x < y,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => false,
    }
}

/// Most salient candidate in `candidates`, or None if empty.
pub fn salient(candidates: &[Candidate]) -> Option<&Candidate> {
    candidates.iter().reduce(|best, c| if more_salient(c, best) { c } else { best })
}

/// Set-based outcome. Deny beats Traverse beats nothing.
pub fn resolve(candidates: &[Candidate]) -> Decision {
    if candidates.iter().any(|c| c.outcome == Outcome::Block) {
        return Decision::Denied;
    }
    let traversers: Vec<Candidate> = candidates
        .iter()
        .filter(|c| matches!(c.outcome, Outcome::Traverse(_)))
        .cloned()
        .collect();
    if let Some(winner) = salient(&traversers) {
        if let Outcome::Traverse(dest) = &winner.outcome {
            return Decision::Allowed { destination: dest.clone() };
        }
    }
    Decision::Unresolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabric::Band;

    fn traverse(band: Band, priority: i32, self_id: Option<&str>, dest: &str) -> Candidate {
        Candidate {
            band,
            priority,
            self_id: self_id.map(|s| EntityId::new(s).unwrap()),
            outcome: Outcome::Traverse(EntityId::new(dest).unwrap()),
            effects: Vec::new(),
        }
    }

    fn block(band: Band, priority: i32, self_id: Option<&str>) -> Candidate {
        Candidate {
            band,
            priority,
            self_id: self_id.map(|s| EntityId::new(s).unwrap()),
            outcome: Outcome::Block,
            effects: Vec::new(),
        }
    }

    #[test]
    fn any_block_denies_even_with_traverse_present() {
        let cands = vec![
            traverse(Band::Structure, 0, None, "snakewood/old-well"),
            block(Band::Participant, 0, Some("snakewood/mob/goblin#1")),
        ];
        assert_eq!(resolve(&cands), Decision::Denied);
    }

    #[test]
    fn lone_traverse_allows_to_destination() {
        let cands = vec![traverse(Band::Structure, 0, None, "snakewood/old-well")];
        assert_eq!(
            resolve(&cands),
            Decision::Allowed { destination: EntityId::new("snakewood/old-well").unwrap() }
        );
    }

    #[test]
    fn empty_is_unresolved() {
        assert_eq!(resolve(&[]), Decision::Unresolved);
    }

    #[test]
    fn salient_prefers_participant_over_structure() {
        let cands = vec![
            block(Band::Structure, 0, None),
            block(Band::Participant, 0, Some("snakewood/mob/goblin#1")),
        ];
        let s = salient(&cands).unwrap();
        assert_eq!(s.band, Band::Participant);
    }

    #[test]
    fn salient_uses_priority_within_band() {
        let cands = vec![
            block(Band::Participant, 1, Some("snakewood/mob/a#1")),
            block(Band::Participant, 5, Some("snakewood/mob/b#1")),
        ];
        let s = salient(&cands).unwrap();
        assert_eq!(s.priority, 5);
    }
}
```

- [ ] **Step 2: Wire into `crates/snakewood-core/src/fabric/mod.rs`**

Add:
```rust
pub mod resolve;
```
and:
```rust
pub use resolve::{resolve, salient, Decision};
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-core fabric::resolve`
Expected: PASS (5 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/fabric/resolve.rs crates/snakewood-core/src/fabric/mod.rs
git commit -m "feat: add set-based resolve and salience selection"
```

---

### Task 9: `dispatch` — Guard + Commit + Notify, Move and Look end-to-end

**Files:**
- Create: `crates/snakewood-core/src/fabric/dispatch.rs`
- Modify: `crates/snakewood-core/src/fabric/mod.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: `crate::{Realm, EntityId, PresentationNode}`, `crate::fabric::{Intent, Event, gather, resolve, salient, Decision, Candidate, Outcome, Effect, Party}`, `crate::world::Room`.
- Produces:
  - `pub struct Dispatch { pub events: Vec<Event>, pub messages: Vec<(EntityId, PresentationNode)> }` deriving `Debug, Clone, PartialEq, Default`.
  - `pub fn dispatch(realm: &mut Realm, intent: Intent) -> Dispatch` — runs Guard→Commit→Notify. Move: Denied → salient blocker's effects as messages, no state change; Allowed → move the actor mob's `location`, emit `Moved`, then append the arrival room presentation to the actor; Unresolved → a `Denied("You see no exit in that direction.")` message. Look → emit `Looked` and the current room's presentation. Notify is a no-op placeholder (documented) in Stage 2.
  - A private `fn room_presentation(realm: &Realm, room_id: &EntityId, viewer: &EntityId) -> Vec<PresentationNode>` shared by Look and Move-arrival (DRY): `RoomName`, `RoomDescription`, `Exits` (sorted directions), `Occupants` (names of other mobs present, sorted).

- [ ] **Step 1: Write `crates/snakewood-core/src/fabric/dispatch.rs` with tests**

```rust
use crate::fabric::{
    gather, resolve, salient, Candidate, Decision, Effect, Event, Intent, Outcome, Party,
};
use crate::{EntityId, PresentationNode, Realm};

/// The result of dispatching one intent: committed events + directed messages.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dispatch {
    pub events: Vec<Event>,
    pub messages: Vec<(EntityId, PresentationNode)>,
}

/// Resolve a Party to a concrete recipient id for an effect.
fn recipient(party: Party, self_id: Option<&EntityId>, actor: &EntityId) -> Option<EntityId> {
    match party {
        Party::Actor => Some(actor.clone()),
        Party::SelfMob => self_id.cloned(),
    }
}

/// Turn a salient candidate's effects into directed messages.
fn apply_effects(cand: &Candidate, actor: &EntityId, out: &mut Vec<(EntityId, PresentationNode)>) {
    for effect in &cand.effects {
        match effect {
            Effect::Narrate(party, text) => {
                if let Some(to) = recipient(*party, cand.self_id.as_ref(), actor) {
                    out.push((to, PresentationNode::Line(text.clone())));
                }
            }
        }
    }
}

/// Build the semantic view of a room for `viewer` (excludes the viewer from occupants).
fn room_presentation(realm: &Realm, room_id: &EntityId, viewer: &EntityId) -> Vec<PresentationNode> {
    let mut nodes = Vec::new();
    if let Some(room) = realm.world.room(room_id) {
        nodes.push(PresentationNode::RoomName(room.name.clone()));
        nodes.push(PresentationNode::RoomDescription(room.description.clone()));
        nodes.push(PresentationNode::Exits(room.exits.keys().cloned().collect()));
        let mut occupants: Vec<String> = realm
            .mobs_in_room(room_id)
            .iter()
            .filter(|m| &m.id != viewer)
            .map(|m| m.name.clone())
            .collect();
        occupants.sort();
        nodes.push(PresentationNode::Occupants(occupants));
    }
    nodes
}

/// Dispatch one intent through Guard -> Commit -> Notify.
pub fn dispatch(realm: &mut Realm, intent: Intent) -> Dispatch {
    let mut out = Dispatch::default();
    let actor = intent.actor().clone();

    match &intent {
        Intent::Look { .. } => {
            // Look is not guarded in Stage 2; it commits a Looked event + view.
            if let Some(room_id) = realm.mob_location(&actor).cloned() {
                out.events.push(Event::Looked { actor: actor.clone(), room: room_id.clone() });
                for node in room_presentation(realm, &room_id, &actor) {
                    out.messages.push((actor.clone(), node));
                }
            }
        }
        Intent::Move { .. } => {
            let from = match realm.mob_location(&actor).cloned() {
                Some(r) => r,
                None => return out, // unlocated actor: nothing happens
            };
            // GUARD
            let candidates = gather(realm, &intent);
            match resolve(&candidates) {
                Decision::Denied => {
                    // salient among the blockers narrates/reacts
                    let blockers: Vec<Candidate> =
                        candidates.iter().filter(|c| c.outcome == Outcome::Block).cloned().collect();
                    if let Some(s) = salient(&blockers) {
                        apply_effects(s, &actor, &mut out.messages);
                    }
                }
                Decision::Allowed { destination } => {
                    // salient traverser may carry effects (usually none for a plain exit)
                    let traversers: Vec<Candidate> = candidates
                        .iter()
                        .filter(|c| matches!(c.outcome, Outcome::Traverse(_)))
                        .cloned()
                        .collect();
                    if let Some(s) = salient(&traversers) {
                        apply_effects(s, &actor, &mut out.messages);
                    }
                    // COMMIT
                    if let Some(mob) = realm.mob_mut(&actor) {
                        mob.location = destination.clone();
                    }
                    out.events.push(Event::Moved {
                        actor: actor.clone(),
                        from,
                        to: destination.clone(),
                    });
                    // arrival view
                    for node in room_presentation(realm, &destination, &actor) {
                        out.messages.push((actor.clone(), node));
                    }
                }
                Decision::Unresolved => {
                    out.messages.push((
                        actor.clone(),
                        PresentationNode::Denied("You see no exit in that direction.".to_string()),
                    ));
                }
            }
            // NOTIFY: Stage 2 publishes events (already in `out`). Observer-driven
            // reactions (mobs emitting follow-up intents) are deferred to Stage 3.
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::fabric::{Party, Predicate, Responder, Trigger};
    use crate::{Direction, Flag, Mob, Room, World};

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
            description: "A crumbling well.".to_string(),
            exits: BTreeMap::new(),
        });
        world
    }

    fn actor_id() -> EntityId {
        EntityId::new("snakewood/pc/nathan").unwrap()
    }

    fn realm_with_actor() -> Realm {
        let mut realm = Realm::new(world_two_rooms());
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        realm.insert_mob(Mob {
            id: actor_id(),
            name: "Nathan".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        });
        realm
    }

    #[test]
    fn move_through_open_exit_relocates_and_emits_moved() {
        let mut realm = realm_with_actor();
        let out = dispatch(&mut realm, Intent::Move { actor: actor_id(), direction: Direction::North });
        assert_eq!(realm.mob_location(&actor_id()).map(|r| r.as_str()), Some("snakewood/old-well"));
        assert!(out.events.contains(&Event::Moved {
            actor: actor_id(),
            from: EntityId::new("snakewood/clearing").unwrap(),
            to: EntityId::new("snakewood/old-well").unwrap(),
        }));
        // arrival view names the new room
        assert!(out.messages.iter().any(|(_, n)| *n == PresentationNode::RoomName("The Old Well".to_string())));
    }

    #[test]
    fn move_with_no_exit_is_denied_with_fallback_message() {
        let mut realm = realm_with_actor();
        let out = dispatch(&mut realm, Intent::Move { actor: actor_id(), direction: Direction::South });
        assert_eq!(realm.mob_location(&actor_id()).map(|r| r.as_str()), Some("snakewood/clearing"));
        assert!(out.messages.iter().any(|(_, n)| *n
            == PresentationNode::Denied("You see no exit in that direction.".to_string())));
        assert!(out.events.is_empty());
    }

    #[test]
    fn look_produces_room_view_and_looked_event() {
        let mut realm = realm_with_actor();
        let out = dispatch(&mut realm, Intent::Look { actor: actor_id() });
        assert!(out.events.contains(&Event::Looked {
            actor: actor_id(),
            room: EntityId::new("snakewood/clearing").unwrap(),
        }));
        assert!(out.messages.iter().any(|(_, n)| *n == PresentationNode::RoomName("Snakewood Clearing".to_string())));
    }
}
```

- [ ] **Step 2: Wire into `mod.rs` and `lib.rs`**

In `crates/snakewood-core/src/fabric/mod.rs` add:
```rust
pub mod dispatch;
```
and:
```rust
pub use dispatch::{dispatch, Dispatch};
```

In `crates/snakewood-core/src/lib.rs` add to the fabric re-export:
```rust
pub use fabric::{dispatch, Dispatch};
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p snakewood-core fabric::dispatch`
Expected: PASS (3 tests: move through exit, no-exit fallback, look).

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-core/src/fabric/dispatch.rs crates/snakewood-core/src/fabric/mod.rs crates/snakewood-core/src/lib.rs
git commit -m "feat: add dispatch (Guard/Commit/Notify) for Move and Look"
```

---

### Task 10: Scenario integration test — the blocking goblin

**Files:**
- Create: `crates/snakewood-core/tests/goblin_scenario.rs`

**Interfaces:**
- Consumes: crate public API — `snakewood_core::{Realm, World, Room, Mob, Flag, EntityId, Direction, Intent, Event, PresentationNode, dispatch}` and `snakewood_core::fabric::{Responder, Rule, Trigger, Predicate, Party, Effect, Outcome}` (ensure these are reachable via `snakewood_core::fabric::...`; they are re-exported there per Tasks 4–8).
- Produces: an integration test proving the headline Stage 2 behavior end-to-end: a live goblin blocks northward movement with the salient goblin message; incapacitating the goblin (removing its `Conscious`/`Alive` flags) lets the same intent succeed — with no change to any subscription wiring, only state.

- [ ] **Step 1: Write `crates/snakewood-core/tests/goblin_scenario.rs`**

```rust
use std::collections::{BTreeMap, BTreeSet};

use snakewood_core::fabric::{Effect, Outcome, Party, Predicate, Responder, Trigger};
use snakewood_core::{
    dispatch, Direction, EntityId, Event, Flag, Intent, Mob, PresentationNode, Realm, Room, World,
};

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
        description: "Gnarled snakewood trees ring a clearing.".to_string(),
        exits,
    });
    world.insert_room(Room {
        id: id("snakewood/old-well"),
        name: "The Old Well".to_string(),
        description: "A crumbling stone well.".to_string(),
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

fn realm() -> Realm {
    let mut realm = Realm::new(world());
    realm.insert_mob(actor());
    realm.insert_mob(goblin());
    realm
}

#[test]
fn conscious_goblin_blocks_north_with_salient_message() {
    let mut realm = realm();
    let out = dispatch(&mut realm, Intent::Move { actor: id("snakewood/pc/nathan"), direction: Direction::North });

    // Actor did NOT move.
    assert_eq!(realm.mob_location(&id("snakewood/pc/nathan")).map(|r| r.as_str()), Some("snakewood/clearing"));
    // No Moved event.
    assert!(!out.events.iter().any(|e| matches!(e, Event::Moved { .. })));
    // The salient (Participant) narration is the goblin's, delivered to the actor.
    assert!(out.messages.contains(&(
        id("snakewood/pc/nathan"),
        PresentationNode::Line("The goblin blocks your way north.".to_string())
    )));
}

#[test]
fn incapacitated_goblin_stops_blocking_no_wiring_change() {
    let mut realm = realm();
    // Knock the goblin unconscious — pure state change, no subscription edits.
    realm.mob_mut(&id("snakewood/mob/goblin#1")).unwrap().flags.remove(&Flag::Conscious);

    let out = dispatch(&mut realm, Intent::Move { actor: id("snakewood/pc/nathan"), direction: Direction::North });

    // Now the actor moves through.
    assert_eq!(realm.mob_location(&id("snakewood/pc/nathan")).map(|r| r.as_str()), Some("snakewood/old-well"));
    assert!(out.events.contains(&Event::Moved {
        actor: id("snakewood/pc/nathan"),
        from: id("snakewood/clearing"),
        to: id("snakewood/old-well"),
    }));
    // The goblin's block message is absent.
    assert!(!out.messages.iter().any(|(_, n)| *n == PresentationNode::Line("The goblin blocks your way north.".to_string())));
}
```

- [ ] **Step 2: Run the integration test**

Run: `cargo test -p snakewood-core --test goblin_scenario`
Expected: PASS (`conscious_goblin_blocks_north_with_salient_message`, `incapacitated_goblin_stops_blocking_no_wiring_change`).

> If the `use snakewood_core::fabric::{...}` path fails to resolve, confirm Tasks 4–8 re-exported those names under `fabric` (in `fabric/mod.rs`). Do NOT weaken the test; fix the missing re-export in the appropriate module and re-run.

- [ ] **Step 3: Commit**

```bash
git add crates/snakewood-core/tests/goblin_scenario.rs
git commit -m "test: prove the blocking-goblin scenario end-to-end"
```

---

### Task 11: Stage-completion verification

**Files:** none (verification only).

**Interfaces:**
- Consumes: the whole crate.
- Produces: confidence Stage 2 is green and clean.

- [ ] **Step 1: Run the full test suite**

Run: `cargo test -p snakewood-core`
Expected: all pass — Stage 1 tests (direction/id/world/serialize/store/roundtrip) plus Stage 2 (mob/realm/fabric::*/goblin_scenario). Confirm the Stage 1 round-trip and golden tests are still green (proves `World` was left frozen).

- [ ] **Step 2: Run clippy and formatting**

Run: `cargo clippy -p snakewood-core --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no diffs. If `cargo fmt --check` reports diffs, run `cargo fmt`, re-run the suite, and commit.

- [ ] **Step 3: Commit any formatting fixes**

```bash
git add -A
git commit -m "chore: stage 2 verification — clippy clean, cargo fmt"
```
(If there was nothing to commit, skip.)

---

## Self-Review

**1. Spec coverage (spec §4, the event fabric):**
- Intent vs. Event — Task 3. ✓
- Guard→Commit→Notify lifecycle — Task 9 (Notify's observer reactions explicitly deferred to Stage 3, documented). ✓
- Two-pass Guard: set-based outcome (`Deny > Allow` lattice) + salience narration (Participant→Structure→Global + priority) — Tasks 7–9. ✓
- Derived + guarded subscription (co-presence + predicates, never stored) — Tasks 2, 7 (proved by `dead_goblin_contributes_nothing_guarded_out` and the goblin scenario). ✓
- Rules (global) + Responders (entity-attached), handlers as data — Task 6. ✓
- Sugar exits as trivial Structure handlers — Task 7. ✓
- Semantic presentation model, no formatted text — Tasks 6, 9. ✓
- Correctly deferred (spec calls these later): Redirect, operators, tick loop, locked doors/room responders, observer-emitted intents, combat, prototypes, persistence of live state. ✓

**2. Placeholder scan:** No "TBD/TODO/implement later". The Notify no-op is an explicit, documented Stage-2 scope decision (deferral), not a placeholder. The integration test's re-export troubleshooting note is a legitimate wiring check, not vague guidance. ✓

**3. Type consistency:** `dispatch(&mut Realm, Intent) -> Dispatch` consistent (Tasks 9–10). `gather(&Realm, &Intent) -> Vec<Candidate>` (Tasks 7–9). `resolve(&[Candidate]) -> Decision` and `salient(&[Candidate]) -> Option<&Candidate>` (Tasks 8–9). `Outcome::{Traverse(EntityId), Block, Allow}`, `Effect::Narrate(Party, String)`, `Predicate::{Alive,Conscious}(Party)`, `Party::{Actor,SelfMob}`, `Band::{Participant,Structure,Global}` used identically across Tasks 5–10. `Trigger::matches(&Intent) -> bool` (Tasks 4, 7). `Mob.responders` and `Realm.rules` added in Task 6 and used in Tasks 7+. Fixture `Mob { .. }` literals updated to include `responders` wherever they appear (Tasks 6 Steps 3–4). ✓

**Cross-task note:** Tasks 1 and 2 deliberately define `Mob`/`Realm` WITHOUT the `responders`/`rules` fields (those types don't exist yet); Task 6 adds them and updates the earlier tests' fixtures. This is called out explicitly in Tasks 1, 2, and 6 so an out-of-order reader isn't surprised.
