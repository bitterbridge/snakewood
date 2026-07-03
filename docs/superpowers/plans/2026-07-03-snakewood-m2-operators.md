# M2 Operators Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the event fabric's operator vocabulary (`RateLimit`, `Coalesce`) by moving the daemon from synchronous per-`submit` dispatch to a deterministic, tick-drained intent queue.

**Architecture:** `snakewood-core` gains a pure operator vocabulary (data, serde/RON) plus two pure evaluators (`RateLimiterState::admit`, `coalesce`), persisted as `world/operators.ron`. `Engine::submit` becomes `Engine::enqueue`; `Engine::tick()` drains the queue each tick — RateLimit gates intents, `dispatch` runs, Coalesce collapses redundant redraws, survivors flush to session outboxes. Transports switch to enqueue and deliver output after a drain (telnet on a per-connection flush; the JSON API waits one drain then responds).

**Tech Stack:** Rust, `serde`/`ron`, `git2`, current-thread `tokio` + `Rc<RefCell<Engine>>` on a `LocalSet`, `proptest`.

## Global Constraints

- **Everything is data, no scripting.** Operators are a fixed Rust-evaluated enum composed as data. Do not add scripting.
- **Determinism.** Operators count the injected **tick counter**, never wall-clock. Outcome is a pure function of `(tick number, ordered intents enqueued before it)`.
- **Test at the cheapest layer.** Core logic and operator evaluation are unit-tested in `snakewood-core`; the socket boundary is not where logic is tested.
- **Injected `Clock`.** `ManualClock` in tests, `SystemClock` in prod. Never call wall-clock directly in logic.
- **Canonical RON.** Use the existing `to_ron`/`from_ron` helpers; one file per authored artifact.
- **Every commit compiles, passes all existing tests, and is `cargo fmt`-clean.** Cheap-tier implementers must run `cargo fmt` before committing. Never use `--no-verify`.
- **`within_ticks > 1` for redraws is out of scope** (§5 hazard in the spec): M2 implements Coalesce as within-a-single-tick-batch collapse only.
- **Instance/id conventions unchanged:** room ids are 2-segment `zone/name`; `world/operators.ron` sits at the `world/` root (parallel to `rules.ron`), and `load_all`'s room filter already ignores non-`rooms` files under `world/`.

---

## Stage 1 — Operator vocabulary, evaluators, persistence (core)

**Goal:** A pure, serde/RON operator vocabulary with two tested evaluators, carried on `Realm` and persisted to `world/operators.ron`. No daemon wiring yet.

### Task 1.1: Operator vocabulary types

**Files:**
- Create: `crates/snakewood-core/src/fabric/operator.rs`
- Modify: `crates/snakewood-core/src/fabric/mod.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: `crate::fabric::Intent`, `crate::PresentationNode`, `crate::EntityId`.
- Produces:
  - `enum IntentClass { Move, Look }` with `fn of(intent: &Intent) -> IntentClass`
  - `enum PresentationKind { RoomName, RoomDescription, Exits, Occupants, Line, Denied, Prompt }` with `fn of(node: &PresentationNode) -> PresentationKind`
  - `enum Scope { Global, PerActor }`
  - `enum Operator { RateLimit { on: IntentClass, per_ticks: u64, scope: Scope, deny: Option<String> }, Coalesce { on: Vec<PresentationKind>, within_ticks: u64, scope: Scope } }`

- [ ] **Step 1: Write the failing test**

Create `crates/snakewood-core/src/fabric/operator.rs` with only a `tests` module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabric::Intent;
    use crate::{Direction, EntityId, PresentationNode};

    #[test]
    fn intent_class_maps_variants() {
        let mv = Intent::Move {
            actor: EntityId::new("player/anon-0").unwrap(),
            direction: Direction::North,
        };
        let look = Intent::Look {
            actor: EntityId::new("player/anon-0").unwrap(),
        };
        assert_eq!(IntentClass::of(&mv), IntentClass::Move);
        assert_eq!(IntentClass::of(&look), IntentClass::Look);
    }

    #[test]
    fn presentation_kind_maps_variants() {
        assert_eq!(
            PresentationKind::of(&PresentationNode::RoomName("x".into())),
            PresentationKind::RoomName
        );
        assert_eq!(
            PresentationKind::of(&PresentationNode::Denied("no".into())),
            PresentationKind::Denied
        );
        assert_eq!(
            PresentationKind::of(&PresentationNode::Prompt),
            PresentationKind::Prompt
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p snakewood-core operator::tests`
Expected: FAIL to compile ("cannot find type `IntentClass`").

- [ ] **Step 3: Write minimal implementation**

At the top of `crates/snakewood-core/src/fabric/operator.rs`, above the tests module:

```rust
use serde::{Deserialize, Serialize};

use crate::fabric::Intent;
use crate::PresentationNode;

/// Which intent class an operator matches. Coarser than `Trigger` — operators
/// gate by class, not by exact direction.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentClass {
    Move,
    Look,
}

impl IntentClass {
    pub fn of(intent: &Intent) -> IntentClass {
        match intent {
            Intent::Move { .. } => IntentClass::Move,
            Intent::Look { .. } => IntentClass::Look,
        }
    }
}

/// The kind of a presentation node, for kind-based coalescing.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PresentationKind {
    RoomName,
    RoomDescription,
    Exits,
    Occupants,
    Line,
    Denied,
    Prompt,
}

impl PresentationKind {
    pub fn of(node: &PresentationNode) -> PresentationKind {
        match node {
            PresentationNode::RoomName(_) => PresentationKind::RoomName,
            PresentationNode::RoomDescription(_) => PresentationKind::RoomDescription,
            PresentationNode::Exits(_) => PresentationKind::Exits,
            PresentationNode::Occupants(_) => PresentationKind::Occupants,
            PresentationNode::Line(_) => PresentationKind::Line,
            PresentationNode::Denied(_) => PresentationKind::Denied,
            PresentationNode::Prompt => PresentationKind::Prompt,
        }
    }
}

/// The scope key an operator's state machine is bucketed by.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    PerActor,
}

/// A declarative stream operator, attached as data on the `Realm`. Each is a
/// deterministic, tick-quantized state machine (state held at runtime by the
/// host). See the M2 design spec §3.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Operator {
    /// Admit at most one matching intent per `per_ticks` window per scope key.
    RateLimit {
        on: IntentClass,
        per_ticks: u64,
        scope: Scope,
        #[serde(default)]
        deny: Option<String>,
    },
    /// Collapse redundant directed-presentation nodes of the listed kinds.
    /// M2 collapses within a single tick's batch (`within_ticks` reserved; see
    /// the M2 design spec §5 for why cross-tick redraw coalescing is unsafe).
    Coalesce {
        on: Vec<PresentationKind>,
        within_ticks: u64,
        scope: Scope,
    },
}
```

- [ ] **Step 4: Wire the module and re-exports**

In `crates/snakewood-core/src/fabric/mod.rs`, add after the other `pub mod` lines:

```rust
pub mod operator;
```

and add to the `pub use` block:

```rust
pub use operator::{IntentClass, Operator, PresentationKind, Scope};
```

In `crates/snakewood-core/src/lib.rs`, extend the fabric re-export line to include the new names:

```rust
pub use fabric::{
    dispatch, Band, Dispatch, Effect, Event, Intent, IntentClass, Operator, Outcome,
    PresentationKind, Responder, Rule, Scope,
};
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p snakewood-core operator::tests`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add crates/snakewood-core/src/fabric/operator.rs crates/snakewood-core/src/fabric/mod.rs crates/snakewood-core/src/lib.rs
git commit -m "feat(core): add operator vocabulary types (RateLimit, Coalesce)"
```

---

### Task 1.2: RON round-trip + proptest for operators

**Files:**
- Modify: `crates/snakewood-core/src/fabric/operator.rs` (tests module)

**Interfaces:**
- Consumes: `crate::{to_ron, from_ron}`, `Operator`, `proptest`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `operator.rs`:

```rust
use crate::{from_ron, to_ron};
use proptest::prelude::*;

#[test]
fn operator_ron_round_trips_and_is_readable() {
    let ops = vec![
        Operator::RateLimit {
            on: IntentClass::Move,
            per_ticks: 3,
            scope: Scope::PerActor,
            deny: Some("Slow down.".to_string()),
        },
        Operator::Coalesce {
            on: vec![PresentationKind::RoomName, PresentationKind::Exits],
            within_ticks: 1,
            scope: Scope::PerActor,
        },
    ];
    let text = to_ron(&ops);
    // Golden-ish: output is human-readable and names the operators.
    assert!(text.contains("RateLimit"), "RON:\n{text}");
    assert!(text.contains("per_ticks: 3"), "RON:\n{text}");
    assert!(text.contains("Coalesce"), "RON:\n{text}");
    // Round-trips losslessly.
    let back: Vec<Operator> = from_ron(&text).unwrap();
    assert_eq!(back, ops);
}

fn arb_intent_class() -> impl Strategy<Value = IntentClass> {
    prop_oneof![Just(IntentClass::Move), Just(IntentClass::Look)]
}

fn arb_scope() -> impl Strategy<Value = Scope> {
    prop_oneof![Just(Scope::Global), Just(Scope::PerActor)]
}

fn arb_kind() -> impl Strategy<Value = PresentationKind> {
    prop_oneof![
        Just(PresentationKind::RoomName),
        Just(PresentationKind::RoomDescription),
        Just(PresentationKind::Exits),
        Just(PresentationKind::Occupants),
        Just(PresentationKind::Line),
        Just(PresentationKind::Denied),
        Just(PresentationKind::Prompt),
    ]
}

fn arb_operator() -> impl Strategy<Value = Operator> {
    prop_oneof![
        (arb_intent_class(), 1u64..20, arb_scope(), any::<Option<String>>()).prop_map(
            |(on, per_ticks, scope, deny)| Operator::RateLimit {
                on,
                per_ticks,
                scope,
                deny
            }
        ),
        (
            prop::collection::vec(arb_kind(), 0..7),
            1u64..5,
            arb_scope()
        )
            .prop_map(|(on, within_ticks, scope)| Operator::Coalesce {
                on,
                within_ticks,
                scope
            }),
    ]
}

proptest! {
    #[test]
    fn any_operator_list_round_trips(ops in prop::collection::vec(arb_operator(), 0..8)) {
        let text = to_ron(&ops);
        let back: Vec<Operator> = from_ron(&text).unwrap();
        prop_assert_eq!(back, ops);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail (or pass trivially)**

Run: `cargo test -p snakewood-core operator::tests`
Expected: COMPILES and PASSES (the types already round-trip). If any assertion about substrings fails, adjust the substring to match `to_ron` output — do NOT change the types.

- [ ] **Step 3: (No implementation needed — types already support this.)**

If `proptest` is not already a dev-dependency of `snakewood-core`, confirm it: `crates/snakewood-core/Cargo.toml` should have `proptest` under `[dev-dependencies]` (it is used by `tests/roundtrip.rs`). No change expected.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p snakewood-core operator::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add crates/snakewood-core/src/fabric/operator.rs
git commit -m "test(core): operator RON round-trip + proptest"
```

---

### Task 1.3: `RateLimiterState::admit` evaluator

**Files:**
- Modify: `crates/snakewood-core/src/fabric/operator.rs`
- Modify: `crates/snakewood-core/src/fabric/mod.rs` (re-export)
- Modify: `crates/snakewood-core/src/lib.rs` (re-export)

**Interfaces:**
- Consumes: `Operator`, `IntentClass`, `Scope`, `EntityId`.
- Produces:
  - `enum Admission { Admit, Drop { deny: Option<String> } }`
  - `struct RateLimiterState` with `fn admit(&mut self, operators: &[Operator], class: IntentClass, actor: &EntityId, tick: u64) -> Admission`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `operator.rs`:

```rust
use crate::EntityId;

fn move_rl(per_ticks: u64, scope: Scope) -> Vec<Operator> {
    vec![Operator::RateLimit {
        on: IntentClass::Move,
        per_ticks,
        scope,
        deny: Some("Slow down.".to_string()),
    }]
}

#[test]
fn rate_limit_admits_one_per_window_then_drops() {
    let ops = move_rl(3, Scope::PerActor);
    let a = EntityId::new("player/anon-0").unwrap();
    let mut rl = RateLimiterState::default();
    // First admission at tick 0 always allowed.
    assert!(matches!(rl.admit(&ops, IntentClass::Move, &a, 0), Admission::Admit));
    // Within the window (ticks 1, 2) -> dropped with the configured deny.
    match rl.admit(&ops, IntentClass::Move, &a, 1) {
        Admission::Drop { deny } => assert_eq!(deny.as_deref(), Some("Slow down.")),
        Admission::Admit => panic!("should be dropped inside window"),
    }
    assert!(matches!(rl.admit(&ops, IntentClass::Move, &a, 2), Admission::Drop { .. }));
    // At tick 3 the window reopens.
    assert!(matches!(rl.admit(&ops, IntentClass::Move, &a, 3), Admission::Admit));
}

#[test]
fn per_actor_scope_has_independent_buckets() {
    let ops = move_rl(3, Scope::PerActor);
    let a = EntityId::new("player/anon-0").unwrap();
    let b = EntityId::new("player/anon-1").unwrap();
    let mut rl = RateLimiterState::default();
    assert!(matches!(rl.admit(&ops, IntentClass::Move, &a, 0), Admission::Admit));
    // b is a different bucket -> admitted at the same tick.
    assert!(matches!(rl.admit(&ops, IntentClass::Move, &b, 0), Admission::Admit));
    // a is still limited.
    assert!(matches!(rl.admit(&ops, IntentClass::Move, &a, 1), Admission::Drop { .. }));
}

#[test]
fn global_scope_shares_one_bucket() {
    let ops = move_rl(3, Scope::Global);
    let a = EntityId::new("player/anon-0").unwrap();
    let b = EntityId::new("player/anon-1").unwrap();
    let mut rl = RateLimiterState::default();
    assert!(matches!(rl.admit(&ops, IntentClass::Move, &a, 0), Admission::Admit));
    // Same global bucket -> b is dropped even though it's a different actor.
    assert!(matches!(rl.admit(&ops, IntentClass::Move, &b, 1), Admission::Drop { .. }));
}

#[test]
fn non_matching_class_is_always_admitted() {
    let ops = move_rl(3, Scope::PerActor);
    let a = EntityId::new("player/anon-0").unwrap();
    let mut rl = RateLimiterState::default();
    // Look is not rate-limited by a Move operator.
    assert!(matches!(rl.admit(&ops, IntentClass::Look, &a, 0), Admission::Admit));
    assert!(matches!(rl.admit(&ops, IntentClass::Look, &a, 0), Admission::Admit));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p snakewood-core operator::tests::rate_limit`
Expected: FAIL to compile ("cannot find type `RateLimiterState`").

- [ ] **Step 3: Write minimal implementation**

Add to `operator.rs` (above the tests module):

```rust
use std::collections::BTreeMap;

use crate::EntityId;

/// The outcome of a rate-limit admission check.
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    Admit,
    Drop { deny: Option<String> },
}

/// Runtime admission state for all `RateLimit` operators. Keyed by
/// `(operator index, scope key)` -> last admitted tick. Held by the host
/// (runtime-only, never persisted).
#[derive(Debug, Default, Clone)]
pub struct RateLimiterState {
    last_admitted: BTreeMap<(usize, Option<EntityId>), u64>,
}

fn scope_key(scope: Scope, actor: &EntityId) -> Option<EntityId> {
    match scope {
        Scope::Global => None,
        Scope::PerActor => Some(actor.clone()),
    }
}

impl RateLimiterState {
    /// Decide whether an intent of `class` by `actor` is admitted at `tick`,
    /// against the `RateLimit` operators in `operators`. Records the admission
    /// (updating internal state) only when admitted. On a block, returns the
    /// first blocking operator's `deny` text.
    pub fn admit(
        &mut self,
        operators: &[Operator],
        class: IntentClass,
        actor: &EntityId,
        tick: u64,
    ) -> Admission {
        // Pass 1: is any matching operator currently blocking?
        for (idx, op) in operators.iter().enumerate() {
            if let Operator::RateLimit {
                on,
                per_ticks,
                scope,
                deny,
            } = op
            {
                if *on != class {
                    continue;
                }
                let key = (idx, scope_key(*scope, actor));
                if let Some(&last) = self.last_admitted.get(&key) {
                    if tick < last + *per_ticks {
                        return Admission::Drop { deny: deny.clone() };
                    }
                }
            }
        }
        // Pass 2: admit — record the admission for every matching operator.
        for (idx, op) in operators.iter().enumerate() {
            if let Operator::RateLimit { on, scope, .. } = op {
                if *on != class {
                    continue;
                }
                self.last_admitted
                    .insert((idx, scope_key(*scope, actor)), tick);
            }
        }
        Admission::Admit
    }
}
```

Add re-exports: in `fabric/mod.rs` extend the operator `pub use` to `pub use operator::{Admission, IntentClass, Operator, PresentationKind, RateLimiterState, Scope};` and in `lib.rs` add `Admission, RateLimiterState` to the fabric re-export line.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p snakewood-core operator::tests`
Expected: PASS (all operator tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add crates/snakewood-core/src/fabric/operator.rs crates/snakewood-core/src/fabric/mod.rs crates/snakewood-core/src/lib.rs
git commit -m "feat(core): RateLimiterState::admit (sliding tick-window admission)"
```

---

### Task 1.4: `coalesce` evaluator

**Files:**
- Modify: `crates/snakewood-core/src/fabric/operator.rs`
- Modify: `crates/snakewood-core/src/fabric/mod.rs` (re-export)
- Modify: `crates/snakewood-core/src/lib.rs` (re-export)

**Interfaces:**
- Consumes: `PresentationNode`, `PresentationKind`.
- Produces: `fn coalesce(nodes: Vec<PresentationNode>, kinds: &[PresentationKind]) -> Vec<PresentationNode>`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `operator.rs`:

```rust
#[test]
fn coalesce_keeps_last_of_each_configured_kind() {
    // Two room redraws in one batch; collapse to the last room's nodes.
    let nodes = vec![
        PresentationNode::RoomName("Clearing".into()),
        PresentationNode::Exits(vec![Direction::North]),
        PresentationNode::RoomName("Old Well".into()),
        PresentationNode::Exits(vec![Direction::South]),
    ];
    let out = coalesce(
        nodes,
        &[PresentationKind::RoomName, PresentationKind::Exits],
    );
    assert_eq!(
        out,
        vec![
            PresentationNode::RoomName("Old Well".into()),
            PresentationNode::Exits(vec![Direction::South]),
        ]
    );
}

#[test]
fn coalesce_leaves_unconfigured_kinds_untouched_and_ordered() {
    let nodes = vec![
        PresentationNode::RoomName("A".into()),
        PresentationNode::Line("hi".into()),
        PresentationNode::RoomName("B".into()),
        PresentationNode::Line("bye".into()),
    ];
    // Only RoomName is coalesced; both Lines survive in order.
    let out = coalesce(nodes, &[PresentationKind::RoomName]);
    assert_eq!(
        out,
        vec![
            PresentationNode::Line("hi".into()),
            PresentationNode::RoomName("B".into()),
            PresentationNode::Line("bye".into()),
        ]
    );
}

#[test]
fn coalesce_with_no_kinds_is_identity() {
    let nodes = vec![
        PresentationNode::RoomName("A".into()),
        PresentationNode::RoomName("B".into()),
    ];
    let out = coalesce(nodes.clone(), &[]);
    assert_eq!(out, nodes);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p snakewood-core operator::tests::coalesce`
Expected: FAIL to compile ("cannot find function `coalesce`").

- [ ] **Step 3: Write minimal implementation**

Add to `operator.rs` (above the tests module):

```rust
/// Collapse redundant presentation nodes within a single batch: for each kind
/// in `kinds`, keep only its LAST occurrence; nodes whose kind is not listed
/// are all kept. Relative order of survivors is preserved.
///
/// M2 applies this per-recipient over one tick's batch only. Do NOT extend it
/// to suppress redraws across ticks (M2 design spec §5): dropping a later-tick
/// `RoomName` because an earlier tick emitted one would hide a room the player
/// actually moved into.
pub fn coalesce(
    nodes: Vec<PresentationNode>,
    kinds: &[PresentationKind],
) -> Vec<PresentationNode> {
    let is_coalesced = |k: PresentationKind| kinds.contains(&k);
    // Index of the last occurrence of each coalesced kind.
    let mut last_index: BTreeMap<PresentationKind, usize> = BTreeMap::new();
    for (i, n) in nodes.iter().enumerate() {
        let k = PresentationKind::of(n);
        if is_coalesced(k) {
            last_index.insert(k, i);
        }
    }
    nodes
        .into_iter()
        .enumerate()
        .filter(|(i, n)| {
            let k = PresentationKind::of(n);
            !is_coalesced(k) || last_index.get(&k) == Some(i)
        })
        .map(|(_, n)| n)
        .collect()
}
```

Add `coalesce` to the re-exports in `fabric/mod.rs` (`pub use operator::{... , coalesce};`) and in `lib.rs`'s fabric re-export line.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p snakewood-core operator::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add crates/snakewood-core/src/fabric/operator.rs crates/snakewood-core/src/fabric/mod.rs crates/snakewood-core/src/lib.rs
git commit -m "feat(core): coalesce evaluator (within-batch last-wins per kind)"
```

---

### Task 1.5: Carry operators on `Realm` + persist to `world/operators.ron`

**Files:**
- Modify: `crates/snakewood-core/src/realm.rs`
- Modify: `crates/snakewood-core/src/store/mod.rs`
- Modify: `crates/snakewood-core/src/store/memory.rs`
- Modify: `crates/snakewood-core/src/store/git.rs`

**Interfaces:**
- Consumes: `Operator`.
- Produces:
  - `Realm.operators: Vec<Operator>` and `Realm.rate_limit_message: String` (default `"You can't do that yet."`)
  - `WorldStore::save_operators(&mut self, operators: &[Operator]) -> Result<(), StoreError>`
  - `WorldStore::load_operators(&self) -> Result<Vec<Operator>, StoreError>`
  - `load_realm`/`save_realm` include operators.

- [ ] **Step 1: Write the failing tests**

In `crates/snakewood-core/src/realm.rs` tests module, add:

```rust
#[test]
fn realm_starts_with_no_operators_and_default_rate_limit_message() {
    let realm = Realm::new(World::default());
    assert!(realm.operators.is_empty());
    assert_eq!(realm.rate_limit_message, "You can't do that yet.");
}
```

In `crates/snakewood-core/src/store/git.rs` tests module, add:

```rust
#[test]
fn operators_round_trip_through_git() {
    use crate::{IntentClass, Operator, Scope};
    let dir = tempdir().unwrap();
    let mut store = GitStore::init(dir.path()).unwrap();
    let mut realm = crate::Realm::new({
        let mut w = World::default();
        w.insert_room(clearing());
        w
    });
    realm.operators.push(Operator::RateLimit {
        on: IntentClass::Move,
        per_ticks: 4,
        scope: Scope::PerActor,
        deny: Some("Slow down.".to_string()),
    });
    store.save_realm(&realm).unwrap();
    store.commit("save realm with operators", 1_700_000_000).unwrap();

    let reloaded = GitStore::init(dir.path()).unwrap().load_realm().unwrap();
    assert_eq!(reloaded.operators, realm.operators);
    // Written at the world/ root, parallel to rules.ron.
    assert!(dir.path().join("world/operators.ron").exists());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p snakewood-core operators_round_trip_through_git realm_starts_with_no_operators`
Expected: FAIL to compile ("no field `operators`" / "no method `save_operators`").

- [ ] **Step 3: Implement `Realm` fields**

In `crates/snakewood-core/src/realm.rs`, add to the struct:

```rust
    /// Declarative stream operators attached to this realm (authored data).
    pub operators: Vec<crate::Operator>,
    /// Message shown when a RateLimit operator drops an intent and the operator
    /// carries no explicit `deny` text. Data, not hardcoded.
    pub rate_limit_message: String,
```

and in `Realm::new`:

```rust
            operators: Vec::new(),
            rate_limit_message: "You can't do that yet.".to_string(),
```

- [ ] **Step 4: Implement the trait methods + `save_realm`/`load_realm`**

In `crates/snakewood-core/src/store/mod.rs`:
- Add `Operator` to the top `use crate::{...}` line.
- Add the two required methods to the `WorldStore` trait (no default), e.g. after `load_rules`:

```rust
    /// Persist the operator list (authored, `world/` in git-backed impls).
    fn save_operators(&mut self, operators: &[Operator]) -> Result<(), StoreError>;

    /// Load the operator list (empty if none persisted).
    fn load_operators(&self) -> Result<Vec<Operator>, StoreError>;
```

- In the default `load_realm`, after `realm.rules = self.load_rules()?;` add:

```rust
        realm.operators = self.load_operators()?;
```

- In the default `save_realm`, after `self.save_rules(&realm.rules)?;` add:

```rust
        self.save_operators(&realm.operators)?;
```

- [ ] **Step 5: Implement `MemoryStore`**

In `crates/snakewood-core/src/store/memory.rs`:
- Add `Operator` to the `use crate::{...}` line and an `operators: Vec<Operator>` field to the struct.
- Add the impls:

```rust
    fn save_operators(&mut self, operators: &[Operator]) -> Result<(), StoreError> {
        self.operators = operators.to_vec();
        Ok(())
    }

    fn load_operators(&self) -> Result<Vec<Operator>, StoreError> {
        Ok(self.operators.clone())
    }
```

- [ ] **Step 6: Implement `GitStore`**

In `crates/snakewood-core/src/store/git.rs`:
- Add `Operator` to the `use crate::{...}` line.
- Add a path helper next to `rules_path`:

```rust
    fn operators_path(&self) -> PathBuf {
        self.root.join("world").join("operators.ron")
    }
```

- Add the impls (mirror `save_rules`/`load_rules`):

```rust
    fn save_operators(&mut self, operators: &[Operator]) -> Result<(), StoreError> {
        let path = self.operators_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        fs::write(&path, to_ron(operators)).map_err(io_err)?;
        Ok(())
    }

    fn load_operators(&self) -> Result<Vec<Operator>, StoreError> {
        let path = self.operators_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&path).map_err(io_err)?;
        from_ron(&text).map_err(|e| StoreError::Parse(e.to_string()))
    }
```

Note: `load_all` already skips non-`rooms` files under `world/`, so `operators.ron` is not mistaken for a room.

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p snakewood-core`
Expected: PASS (all core tests, including the two new ones).

- [ ] **Step 8: Commit**

```bash
cargo fmt
git add crates/snakewood-core/src/realm.rs crates/snakewood-core/src/store/
git commit -m "feat(core): carry operators on Realm and persist to world/operators.ron"
```

---

## Stage 2 — Engine intent queue + tick drain

**Goal:** `Engine` gains an intent queue, a per-tick drain (RateLimit → dispatch → Coalesce → flush), a drain counter, and pending-session enumeration. `submit` is kept unchanged so existing transports and tests stay green; it is retired in Stage 3.

### Task 2.1: Enqueue, drain counter, pending-session enumeration

**Files:**
- Modify: `crates/snakewood-daemon/src/engine.rs`

**Interfaces:**
- Consumes: `snakewood_core::{IntentClass, Admission, RateLimiterState, Operator, PresentationKind, coalesce, dispatch}`.
- Produces (on `Engine`):
  - `fn enqueue(&mut self, id: SessionId, intent: Intent)` — authorize (session owns actor) then push to the queue.
  - `fn drain_count(&self) -> u64`
  - `fn sessions_with_pending(&self) -> Vec<SessionId>`
  - New fields: `intent_queue: Vec<(SessionId, Intent)>`, `rate_limiter: RateLimiterState`, `drain_count: u64`.

- [ ] **Step 1: Write the failing test**

In `crates/snakewood-daemon/src/engine.rs` tests module, add (uses the existing `engine_with_actor` helper):

```rust
#[test]
fn enqueue_authorizes_and_buffers_without_dispatching() {
    let (mut e, sid, actor) = engine_with_actor();
    // Enqueue does not dispatch: position unchanged, outbox empty, queue holds 1.
    e.enqueue(
        sid,
        Intent::Move {
            actor: actor.clone(),
            direction: Direction::North,
        },
    );
    assert_eq!(
        e.realm().mob_location(&actor).map(|r| r.as_str()),
        Some("snakewood/clearing")
    );
    assert!(e.poll(sid).is_empty());
    assert_eq!(e.drain_count(), 0);
}

#[test]
fn enqueue_rejects_foreign_actor() {
    let (mut e, sid, _actor) = engine_with_actor();
    let stranger = EntityId::new("snakewood/pc/stranger").unwrap();
    e.enqueue(
        sid,
        Intent::Move {
            actor: stranger,
            direction: Direction::North,
        },
    );
    // Nothing queued: a later tick drains nothing and drain_count still advances by 1.
    let before = e.drain_count();
    e.tick();
    assert_eq!(e.drain_count(), before + 1);
    // The unauthorized move never happened.
    assert!(e.sessions_with_pending().is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p snakewood-daemon enqueue_authorizes`
Expected: FAIL to compile ("no method `enqueue`").

- [ ] **Step 3: Add fields**

In `crates/snakewood-daemon/src/engine.rs`:
- Extend the top `use snakewood_core::{...}` to include `coalesce, Admission, IntentClass, Operator, PresentationKind, RateLimiterState` (keep the existing names).
- Add fields to the `Engine` struct:

```rust
    intent_queue: Vec<(SessionId, Intent)>,
    rate_limiter: RateLimiterState,
    drain_count: u64,
```

- Initialize them in `Engine::new`:

```rust
            intent_queue: Vec::new(),
            rate_limiter: RateLimiterState::default(),
            drain_count: 0,
```

- [ ] **Step 4: Add `enqueue`, `drain_count`, `sessions_with_pending`**

Add methods to `impl Engine` (place `enqueue` near `submit`):

```rust
    /// Authorize and buffer an intent for the next tick's drain. Like `submit`,
    /// a session may only enqueue intents for the actor it is bound to.
    pub fn enqueue(&mut self, id: SessionId, intent: Intent) {
        let authorized = matches!(self.sessions.get(&id), Some(s) if &s.actor == intent.actor());
        if !authorized {
            return;
        }
        self.intent_queue.push((id, intent));
    }

    /// How many times the intent queue has been drained (one per `tick`).
    pub fn drain_count(&self) -> u64 {
        self.drain_count
    }

    /// Sessions whose outbox currently holds undelivered presentation.
    pub fn sessions_with_pending(&self) -> Vec<SessionId> {
        self.sessions
            .iter()
            .filter(|(_, s)| !s.outbox.is_empty())
            .map(|(id, _)| *id)
            .collect()
    }
```

- [ ] **Step 5: Make `tick` increment the drain counter (drain body added in Task 2.2)**

For now, change `tick` to also bump `drain_count` so the "rejects foreign actor" test passes. Replace the body of `tick`:

```rust
    pub fn tick(&mut self) -> u64 {
        self.tick += 1;
        self.drain();
        self.tick
    }
```

and add a temporary minimal `drain` (fully implemented in Task 2.2):

```rust
    fn drain(&mut self) {
        let _queue = std::mem::take(&mut self.intent_queue);
        self.drain_count += 1;
    }
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p snakewood-daemon`
Expected: PASS (all daemon tests, including the two new ones). Existing `submit`-based tests are unaffected because `submit` still dispatches immediately and the queue is empty.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add crates/snakewood-daemon/src/engine.rs
git commit -m "feat(daemon): Engine intent queue, enqueue, drain counter, pending-session enum"
```

---

### Task 2.2: Full drain — RateLimit, dispatch, Coalesce, flush

**Files:**
- Modify: `crates/snakewood-daemon/src/engine.rs`

**Interfaces:**
- Consumes: everything from Task 2.1 plus `dispatch`, `coalesce`, `Admission`, `IntentClass`, `Operator`, `PresentationKind`.
- Produces: a complete `drain()` that gates, dispatches, coalesces, and flushes to outboxes; unchanged public signatures.

- [ ] **Step 1: Write the failing tests**

In `crates/snakewood-daemon/src/engine.rs` tests module, add a helper and tests. The helper attaches operators and enqueues N moves:

```rust
use snakewood_core::{IntentClass, Operator, PresentationKind, Scope};

fn engine_with_actor_and_ops(ops: Vec<Operator>) -> (Engine, SessionId, EntityId) {
    let (mut e, sid, actor) = engine_with_actor();
    e.realm_mut().operators = ops;
    (e, sid, actor)
}

#[test]
fn drain_rate_limits_moves_across_ticks() {
    // north then south is a round trip between the two rooms.
    let ops = vec![Operator::RateLimit {
        on: IntentClass::Move,
        per_ticks: 2,
        scope: Scope::PerActor,
        deny: Some("Too fast.".to_string()),
    }];
    let (mut e, sid, actor) = engine_with_actor_and_ops(ops);

    // Tick 1: enqueue two moves (N then S). Only the first is admitted.
    e.enqueue(sid, Intent::Move { actor: actor.clone(), direction: Direction::North });
    e.enqueue(sid, Intent::Move { actor: actor.clone(), direction: Direction::South });
    e.tick();
    // First move (north) committed; second dropped -> still at old-well.
    assert_eq!(
        e.realm().mob_location(&actor).map(|r| r.as_str()),
        Some("snakewood/old-well")
    );
    let out = e.poll(sid);
    // The dropped move produced a Denied node with the configured text.
    assert!(out.iter().any(|n| *n == PresentationNode::Denied("Too fast.".to_string())));
}

#[test]
fn drain_coalesces_repeated_room_views_in_one_tick() {
    // No rate limit; coalesce room-view kinds. Two Looks in one tick each emit a
    // full room view; they collapse to one. (Uses Look, not Move, so it works
    // with the one-way two-room test world.)
    let ops = vec![Operator::Coalesce {
        on: vec![
            PresentationKind::RoomName,
            PresentationKind::RoomDescription,
            PresentationKind::Exits,
            PresentationKind::Occupants,
        ],
        within_ticks: 1,
        scope: Scope::PerActor,
    }];
    let (mut e, sid, actor) = engine_with_actor_and_ops(ops);
    e.enqueue(sid, Intent::Look { actor: actor.clone() });
    e.enqueue(sid, Intent::Look { actor: actor.clone() });
    e.tick();
    let out = e.poll(sid);
    // Exactly one RoomName survives, naming the actor's room.
    let room_names: Vec<&PresentationNode> = out
        .iter()
        .filter(|n| matches!(n, PresentationNode::RoomName(_)))
        .collect();
    assert_eq!(room_names.len(), 1, "views not coalesced: {out:?}");
    assert_eq!(
        room_names[0],
        &PresentationNode::RoomName("Snakewood Clearing".to_string())
    );
}

#[test]
fn drain_with_no_operators_dispatches_normally() {
    let (mut e, sid, actor) = engine_with_actor();
    e.enqueue(sid, Intent::Move { actor: actor.clone(), direction: Direction::North });
    e.tick();
    assert_eq!(
        e.realm().mob_location(&actor).map(|r| r.as_str()),
        Some("snakewood/old-well")
    );
    assert!(e.poll(sid).iter().any(|n| *n == PresentationNode::RoomName("The Old Well".to_string())));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p snakewood-daemon drain_`
Expected: FAIL (the temporary `drain` neither dispatches nor flushes; positions/outboxes are wrong).

- [ ] **Step 3: Replace `drain` with the full implementation**

Replace the temporary `drain` from Task 2.1 with:

```rust
    /// Drain the intent queue for the current tick: gate each intent through
    /// RateLimit operators, dispatch the admitted ones, coalesce the resulting
    /// directed presentation per recipient, and flush to session outboxes.
    fn drain(&mut self) {
        let queue = std::mem::take(&mut self.intent_queue);
        let tick = self.tick;
        let mut batched: Vec<(EntityId, PresentationNode)> = Vec::new();

        for (_sid, intent) in queue {
            let actor = intent.actor().clone();
            let class = IntentClass::of(&intent);
            match self
                .rate_limiter
                .admit(&self.realm.operators, class, &actor, tick)
            {
                Admission::Admit => {
                    let result = dispatch(&mut self.realm, intent);
                    batched.extend(result.messages);
                    // Notify broadcast to bystanders is deferred to M3; events
                    // stay in `result.events` unused here.
                }
                Admission::Drop { deny } => {
                    let text = deny.unwrap_or_else(|| self.realm.rate_limit_message.clone());
                    batched.push((actor, PresentationNode::Denied(text)));
                }
            }
        }

        // Kinds any Coalesce operator targets (union across all Coalesce ops).
        let coalesced_kinds: Vec<PresentationKind> = self
            .realm
            .operators
            .iter()
            .filter_map(|op| match op {
                Operator::Coalesce { on, .. } => Some(on.clone()),
                _ => None,
            })
            .flatten()
            .collect();

        // Distinct recipients in first-seen order (deterministic).
        let mut recipients: Vec<EntityId> = Vec::new();
        for (r, _) in &batched {
            if !recipients.contains(r) {
                recipients.push(r.clone());
            }
        }

        for recipient in recipients {
            let nodes: Vec<PresentationNode> = batched
                .iter()
                .filter(|(r, _)| *r == recipient)
                .map(|(_, n)| n.clone())
                .collect();
            let nodes = if coalesced_kinds.is_empty() {
                nodes
            } else {
                coalesce(nodes, &coalesced_kinds)
            };
            for session in self.sessions.values_mut() {
                if session.actor == recipient {
                    session.outbox.extend(nodes.iter().cloned());
                }
            }
        }

        self.drain_count += 1;
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p snakewood-daemon`
Expected: PASS (all daemon tests). If a borrow error appears on `self.rate_limiter.admit(&self.realm.operators, ...)`, confirm you did not also hold a `&mut self.realm` across it — the disjoint field borrows are legal as written.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add crates/snakewood-daemon/src/engine.rs
git commit -m "feat(daemon): tick drain — RateLimit gate, dispatch, Coalesce, flush"
```

---

## Stage 3 — Heartbeat + transport delivery restructure

**Goal:** Fast configurable heartbeat drives the drain; transports switch from `submit` to `enqueue` and deliver output after a drain. `Engine::submit` is removed. All e2e tests spawn a tick loop and stay green.

### Task 3.1: Heartbeat cadence (`SNAKEWOOD_TICK_MS`)

**Files:**
- Modify: `crates/snakewood-daemon/src/telnet/tick.rs`
- Modify: `crates/snakewood-daemon/src/main.rs`

**Interfaces:**
- Produces: `run_tick_loop(engine: Rc<RefCell<Engine>>, period: Duration)` (signature change from `period_secs: u64`).

- [ ] **Step 1: Change `run_tick_loop` to take a `Duration`**

In `crates/snakewood-daemon/src/telnet/tick.rs`, replace the function:

```rust
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::Engine;

/// Advance the world once per `period` and take interval snapshots. The tick is
/// both the game quantum and the command-processing beat: each tick drains the
/// intent queue. Snapshot cadence is independent (clock-gated in `maybe_snapshot`).
pub async fn run_tick_loop(engine: Rc<RefCell<Engine>>, period: Duration) {
    let mut interval = tokio::time::interval(period);
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

- [ ] **Step 2: Update `main.rs` to read `SNAKEWOOD_TICK_MS`**

In `crates/snakewood-daemon/src/main.rs`:
- Add `use std::time::Duration;` to the imports.
- After the `api_addr` line, add:

```rust
    let tick_ms: u64 = std::env::var("SNAKEWOOD_TICK_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(250);
```

- Replace the `spawn_local(run_tick_loop(engine.clone(), 1));` line with:

```rust
        tokio::task::spawn_local(run_tick_loop(engine.clone(), Duration::from_millis(tick_ms)));
```

- [ ] **Step 3: Verify it builds**

Run: `cargo build -p snakewood-daemon`
Expected: SUCCESS. (Tests that call the old signature are updated in Tasks 3.2/3.3.)

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add crates/snakewood-daemon/src/telnet/tick.rs crates/snakewood-daemon/src/main.rs
git commit -m "feat(daemon): configurable heartbeat (SNAKEWOOD_TICK_MS, default 250ms)"
```

---

### Task 3.2: Telnet reader/writer restructure (enqueue + flush)

**Files:**
- Modify: `crates/snakewood-daemon/src/telnet/server.rs`
- Modify: `crates/snakewood-daemon/tests/telnet_e2e.rs`

**Interfaces:**
- Consumes: `Engine::enqueue`, `Engine::poll`, `run_tick_loop`.
- Produces: a `handle_connection` that reads via a cancel-safe channel and flushes the outbox on a per-connection interval.

- [ ] **Step 1: Update the telnet e2e test to run a tick loop, then confirm it fails without the restructure**

In `crates/snakewood-daemon/tests/telnet_e2e.rs`:
- Add imports: `use std::time::Duration;` (already present) and change the serve import line to `use snakewood_daemon::telnet::{run_tick_loop, serve};`.
- Immediately after the `spawn_local(serve(...))` line, add:

```rust
        tokio::task::spawn_local(run_tick_loop(engine.clone(), Duration::from_millis(20)));
```

  Note: `serve` takes `engine` by value today; clone it first — change the two lines to:

```rust
        tokio::task::spawn_local(serve(listener, engine.clone(), id("snakewood/clearing")));
        tokio::task::spawn_local(run_tick_loop(engine, Duration::from_millis(20)));
```

- Bump the three `read_for(&mut client, 300)` calls to `read_for(&mut client, 500)` (delivery is now bounded by heartbeat + flush).

- [ ] **Step 2: Run the e2e test to verify it fails**

Run: `cargo test -p snakewood-daemon --test telnet_e2e`
Expected: FAIL — the greeting never arrives, because `serve` still calls `submit`+`poll` inline while output now requires a drain. (This confirms the restructure is needed.)

- [ ] **Step 3: Rewrite `handle_connection`**

Replace the body of `handle_connection` in `crates/snakewood-daemon/src/telnet/server.rs` with the channel-reader + flush-interval design. Replace the whole file's `handle_connection` function with:

```rust
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
```

(No change needed to `serve`.)

- [ ] **Step 4: Run the e2e test to verify it passes**

Run: `cargo test -p snakewood-daemon --test telnet_e2e`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add crates/snakewood-daemon/src/telnet/server.rs crates/snakewood-daemon/tests/telnet_e2e.rs
git commit -m "feat(daemon): telnet reader/writer restructure (enqueue + flush delivery)"
```

---

### Task 3.3: JSON API deferred-response restructure + retire `submit`

**Files:**
- Modify: `crates/snakewood-daemon/src/api/handler.rs`
- Modify: `crates/snakewood-daemon/src/api/server.rs`
- Modify: `crates/snakewood-daemon/src/api/mod.rs` (re-exports)
- Modify: `crates/snakewood-daemon/src/engine.rs` (delete `submit`)
- Modify: `crates/snakewood-daemon/tests/api_e2e.rs`
- Modify: `crates/snakewood-daemon/tests/mcp_bridge.rs`

**Interfaces:**
- Produces:
  - `enum ReplyShape { Connected { actor: String }, Messages }`
  - `enum ApiOutcome { Ready(ApiResponse), AwaitDrain { session: SessionId, before: u64, shape: ReplyShape } }`
  - `fn handle_api_request(engine: &mut Engine, req: ApiRequest, start_room: &EntityId) -> ApiOutcome`
  - `fn build_drain_response(shape: ReplyShape, session: SessionId, view: Vec<PresentationNode>) -> ApiResponse`

- [ ] **Step 1: Rewrite `handle_api_request` to return `ApiOutcome`**

In `crates/snakewood-daemon/src/api/handler.rs`, replace the top imports and the whole `handle_api_request` function (keep `actor_of`). New imports:

```rust
use snakewood_core::{EntityId, Intent, PresentationNode};

use crate::api::{ApiRequest, ApiResponse};
use crate::telnet::{attach_named, despawn_player, spawn_player};
use crate::{Engine, SessionId};
```

Add the new types and functions:

```rust
/// How a deferred API reply is shaped once the drain produces the view.
#[derive(Debug, Clone)]
pub enum ReplyShape {
    Connected { actor: String },
    Messages,
}

/// The synchronous result of beginning an API request. Intent-bearing requests
/// enqueue and must await a drain before their view exists.
#[derive(Debug)]
pub enum ApiOutcome {
    Ready(ApiResponse),
    AwaitDrain {
        session: SessionId,
        before: u64,
        shape: ReplyShape,
    },
}

/// Begin handling a structured API request. Control ops (Dig error/Disconnect,
/// bad input) return `Ready`; intent-bearing ops enqueue an intent and return
/// `AwaitDrain` so the caller can wait one drain, then build the reply.
pub fn handle_api_request(
    engine: &mut Engine,
    req: ApiRequest,
    start_room: &EntityId,
) -> ApiOutcome {
    match req {
        ApiRequest::Connect => {
            let (sid, actor) = spawn_player(engine, start_room);
            let before = engine.drain_count();
            engine.enqueue(sid, Intent::Look { actor: actor.clone() });
            ApiOutcome::AwaitDrain {
                session: sid,
                before,
                shape: ReplyShape::Connected {
                    actor: actor.to_string(),
                },
            }
        }
        ApiRequest::ConnectAs { actor } => {
            let actor_id = match EntityId::new(actor.clone()) {
                Ok(id) => id,
                Err(_) => {
                    return ApiOutcome::Ready(ApiResponse::Error {
                        message: format!("invalid actor id: {actor}"),
                    })
                }
            };
            let sid = attach_named(engine, &actor_id, start_room);
            let before = engine.drain_count();
            engine.enqueue(sid, Intent::Look { actor: actor_id.clone() });
            ApiOutcome::AwaitDrain {
                session: sid,
                before,
                shape: ReplyShape::Connected {
                    actor: actor_id.to_string(),
                },
            }
        }
        ApiRequest::Look { session } => {
            let actor = match actor_of(engine, session) {
                Ok(a) => a,
                Err(e) => return ApiOutcome::Ready(e),
            };
            let before = engine.drain_count();
            engine.enqueue(SessionId(session), Intent::Look { actor });
            ApiOutcome::AwaitDrain {
                session: SessionId(session),
                before,
                shape: ReplyShape::Messages,
            }
        }
        ApiRequest::Move { session, direction } => {
            let actor = match actor_of(engine, session) {
                Ok(a) => a,
                Err(e) => return ApiOutcome::Ready(e),
            };
            let before = engine.drain_count();
            engine.enqueue(SessionId(session), Intent::Move { actor, direction });
            ApiOutcome::AwaitDrain {
                session: SessionId(session),
                before,
                shape: ReplyShape::Messages,
            }
        }
        ApiRequest::Dig {
            session,
            direction,
            id,
            name,
            description,
        } => match engine.dig(SessionId(session), direction, &id, &name, &description) {
            Ok(_) => {
                // Show the updated room after the dig (delivered post-drain).
                if let Some(actor) = engine.session_actor(SessionId(session)).cloned() {
                    let before = engine.drain_count();
                    engine.enqueue(SessionId(session), Intent::Look { actor });
                    ApiOutcome::AwaitDrain {
                        session: SessionId(session),
                        before,
                        shape: ReplyShape::Messages,
                    }
                } else {
                    ApiOutcome::Ready(ApiResponse::Ok {
                        messages: Vec::new(),
                    })
                }
            }
            Err(e) => ApiOutcome::Ready(ApiResponse::Error {
                message: format!("dig failed: {e:?}"),
            }),
        },
        ApiRequest::Disconnect { session } => {
            if let Some(actor) = engine.session_actor(SessionId(session)).cloned() {
                despawn_player(engine, SessionId(session), &actor);
            }
            ApiOutcome::Ready(ApiResponse::Ok {
                messages: Vec::new(),
            })
        }
    }
}

/// Build the final response for a deferred request once its view is polled.
pub fn build_drain_response(
    shape: ReplyShape,
    session: SessionId,
    view: Vec<PresentationNode>,
) -> ApiResponse {
    match shape {
        ReplyShape::Connected { actor } => ApiResponse::Connected {
            session: session.0,
            actor,
            view,
        },
        ReplyShape::Messages => ApiResponse::Ok { messages: view },
    }
}
```

- [ ] **Step 2: Rewrite the handler unit tests to drive the drain manually**

Replace the `tests` module in `handler.rs` (keep the `engine()` and `start()` helpers). The pattern for each intent-bearing request: call `handle_api_request` → expect `AwaitDrain` → `e.tick()` → `e.poll(session)` → `build_drain_response` → assert.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualClock;
    use snakewood_core::{Direction, PresentationNode, Realm};

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

    /// Begin a request, drive one drain, and build the final response.
    fn run(e: &mut Engine, req: ApiRequest) -> ApiResponse {
        match handle_api_request(e, req, &start()) {
            ApiOutcome::Ready(r) => r,
            ApiOutcome::AwaitDrain { session, before, shape } => {
                e.tick();
                assert!(e.drain_count() > before);
                let view = e.poll(session);
                build_drain_response(shape, session, view)
            }
        }
    }

    #[test]
    fn connect_returns_session_and_start_room_view() {
        let mut e = engine();
        match run(&mut e, ApiRequest::Connect) {
            ApiResponse::Connected { session, actor, view } => {
                assert_eq!(actor, "player/anon-0");
                assert_eq!(session, 0);
                assert!(view.contains(&PresentationNode::RoomName("Snakewood Clearing".to_string())));
            }
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[test]
    fn connect_as_attaches_named_builder() {
        let mut e = engine();
        match run(&mut e, ApiRequest::ConnectAs { actor: "player/mcp-builder".to_string() }) {
            ApiResponse::Connected { actor, view, .. } => {
                assert_eq!(actor, "player/mcp-builder");
                assert!(view.contains(&PresentationNode::RoomName("Snakewood Clearing".to_string())));
            }
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[test]
    fn move_returns_new_room_view() {
        let mut e = engine();
        let ApiResponse::Connected { session, .. } = run(&mut e, ApiRequest::Connect) else {
            panic!("connect failed");
        };
        match run(&mut e, ApiRequest::Move { session, direction: Direction::North }) {
            ApiResponse::Ok { messages } => {
                assert!(messages.contains(&PresentationNode::RoomName("The Old Well".to_string())));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn dig_then_look_shows_new_exit() {
        let mut e = engine();
        let ApiResponse::Connected { session, .. } = run(&mut e, ApiRequest::Connect) else {
            panic!("connect failed");
        };
        match run(&mut e, ApiRequest::Dig {
            session,
            direction: Direction::East,
            id: "snakewood/hollow".to_string(),
            name: "A Hollow".to_string(),
            description: "Mossy.".to_string(),
        }) {
            ApiResponse::Ok { messages } => {
                assert!(messages.iter().any(|n| matches!(n, PresentationNode::Exits(dirs) if dirs.contains(&Direction::East))));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn unknown_session_is_error() {
        let mut e = engine();
        match handle_api_request(&mut e, ApiRequest::Look { session: 999 }, &start()) {
            ApiOutcome::Ready(ApiResponse::Error { .. }) => {}
            other => panic!("expected Ready(Error), got {other:?}"),
        }
    }

    #[test]
    fn connect_ids_are_distinct_across_calls() {
        let mut e = engine();
        let ApiResponse::Connected { actor: actor1, .. } = run(&mut e, ApiRequest::Connect) else {
            panic!("connect failed");
        };
        let ApiResponse::Connected { actor: actor2, .. } = run(&mut e, ApiRequest::Connect) else {
            panic!("connect failed");
        };
        assert_eq!(actor1, "player/anon-0");
        assert_eq!(actor2, "player/anon-1");
        assert_ne!(actor1, actor2);
    }
}
```

- [ ] **Step 3: Export the new types from the api module**

In `crates/snakewood-daemon/src/api/mod.rs`, replace the line `pub use handler::handle_api_request;` with:

```rust
pub use handler::{build_drain_response, handle_api_request, ApiOutcome, ReplyShape};
```

- [ ] **Step 4: Rewrite `handle_api_connection` to await the drain**

In `crates/snakewood-daemon/src/api/server.rs`:
- Update imports to include the new symbols and `Duration`:

```rust
use std::time::Duration;

use crate::api::{build_drain_response, handle_api_request, ApiOutcome, ApiRequest, ApiResponse};
```

- Add a drain-wait helper above `handle_api_connection`:

```rust
/// Poll until the engine has completed at least one drain past `before`, or a
/// safety timeout elapses (a stuck tick loop must not hang the connection).
async fn wait_for_drain(engine: &Rc<RefCell<Engine>>, before: u64) {
    for _ in 0..300 {
        if engine.borrow().drain_count() > before {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
```

- Replace the request-handling body inside the `while let Some(line)` loop. The classification (`RequestKind`) stays; capture the session from the outcome for cleanup tracking:

```rust
            let parsed = serde_json::from_str::<ApiRequest>(&line);
            let response = match parsed {
                Ok(req) => {
                    let kind = match &req {
                        ApiRequest::Connect => RequestKind::Ephemeral,
                        ApiRequest::ConnectAs { .. } => RequestKind::Persistent,
                        ApiRequest::Disconnect { session } => {
                            RequestKind::Disconnected(SessionId(*session))
                        }
                        _ => RequestKind::Other,
                    };
                    let outcome = {
                        let mut e = engine.borrow_mut();
                        handle_api_request(&mut e, req, &start_room)
                    };
                    // Track any session this request created, for cleanup.
                    if let ApiOutcome::AwaitDrain { session, .. } = &outcome {
                        match kind {
                            RequestKind::Ephemeral => ephemeral.push(*session),
                            RequestKind::Persistent => persistent.push(*session),
                            _ => {}
                        }
                    }
                    if let RequestKind::Disconnected(sid) = kind {
                        ephemeral.retain(|s| *s != sid);
                        persistent.retain(|s| *s != sid);
                    }
                    match outcome {
                        ApiOutcome::Ready(resp) => resp,
                        ApiOutcome::AwaitDrain { session, before, shape } => {
                            wait_for_drain(&engine, before).await;
                            let view = {
                                let mut e = engine.borrow_mut();
                                e.poll(session)
                            };
                            build_drain_response(shape, session, view)
                        }
                    }
                }
                Err(err) => ApiResponse::Error {
                    message: format!("bad request: {err}"),
                },
            };
```

(The `write_half.write_all(...)` block after this and the cleanup block at the end are unchanged.)

- [ ] **Step 5: Delete `Engine::submit` and convert its unit tests**

In `crates/snakewood-daemon/src/engine.rs`, remove the now-unused `submit` method entirely (the doc comment above it too). All production callers now use `enqueue`.

Then convert the five `submit`-based unit tests to `enqueue` + `tick` + `poll`. Confirm none remain: `rg "\.submit\(" crates/snakewood-daemon` must return nothing.

Replace `submit_move_routes_arrival_view_to_session_and_relocates`, `submit_move_no_exit_routes_fallback_message`, `submit_on_unknown_session_is_noop`, and `submit_ignores_intent_acting_as_a_different_actor` with:

```rust
    #[test]
    fn drain_move_routes_arrival_view_to_session_and_relocates() {
        let (mut e, sid, actor) = engine_with_actor();
        e.enqueue(
            sid,
            Intent::Move {
                actor: actor.clone(),
                direction: Direction::North,
            },
        );
        e.tick();
        assert_eq!(
            e.realm().mob_location(&actor).map(|r| r.as_str()),
            Some("snakewood/old-well")
        );
        let view = e.poll(sid);
        assert!(view.contains(&PresentationNode::RoomName("The Old Well".to_string())));
        // draining leaves the outbox empty
        assert!(e.poll(sid).is_empty());
    }

    #[test]
    fn drain_move_no_exit_routes_fallback_message() {
        let (mut e, sid, actor) = engine_with_actor();
        e.enqueue(
            sid,
            Intent::Move {
                actor,
                direction: Direction::South,
            },
        );
        e.tick();
        let view = e.poll(sid);
        assert!(view.contains(&PresentationNode::Denied(
            "You see no exit in that direction.".to_string()
        )));
    }

    #[test]
    fn enqueue_on_unknown_session_is_noop() {
        let (mut e, _sid, actor) = engine_with_actor();
        e.enqueue(SessionId(999), Intent::Look { actor });
        e.tick();
        assert!(e.poll(SessionId(999)).is_empty());
    }

    #[test]
    fn enqueue_ignores_intent_acting_as_a_different_actor() {
        // A session bound to "nathan" cannot drive some other actor.
        let (mut e, sid, _actor) = engine_with_actor();
        let other = EntityId::new("snakewood/pc/impostor").unwrap();
        e.enqueue(
            sid,
            Intent::Move {
                actor: other.clone(),
                direction: Direction::North,
            },
        );
        e.tick();
        assert_eq!(
            e.realm()
                .mob_location(&EntityId::new("snakewood/pc/nathan").unwrap())
                .map(|r| r.as_str()),
            Some("snakewood/clearing")
        );
        assert!(e.realm().mob_location(&other).is_none());
        assert!(e.poll(sid).is_empty());
    }
```

In `checkpoint_persists_live_state_across_a_restart`, replace the `e.submit(...)` move block with an enqueue + drain before the checkpoint:

```rust
            let sid = e.connect(EntityId::new("snakewood/pc/nathan").unwrap());
            e.enqueue(
                sid,
                Intent::Move {
                    actor: EntityId::new("snakewood/pc/nathan").unwrap(),
                    direction: Direction::North,
                },
            );
            e.tick();
            e.checkpoint("player moved north").unwrap();
```

- [ ] **Step 6: Update the API and MCP e2e harnesses to run a tick loop**

In `crates/snakewood-daemon/tests/api_e2e.rs`:
- Change the api import to `use snakewood_daemon::api::{serve_api, ApiRequest, ApiResponse};` (unchanged) and add `use snakewood_daemon::telnet::run_tick_loop;`.
- Replace the `spawn_local(serve_api(...))` line with:

```rust
        tokio::task::spawn_local(serve_api(listener, engine.clone(), id("snakewood/clearing")));
        tokio::task::spawn_local(run_tick_loop(engine, Duration::from_millis(20)));
```

In `crates/snakewood-daemon/tests/mcp_bridge.rs`:
- Add `use snakewood_daemon::telnet::run_tick_loop;` and `use std::time::Duration;`.
- Inside the `local.block_on` async block, replace the final `serve_api(listener, engine, ...).await;` with a clone + spawned tick loop + serve:

```rust
            tokio::task::spawn_local(run_tick_loop(engine.clone(), Duration::from_millis(20)));
            serve_api(listener, engine, id("snakewood/clearing")).await;
```

- [ ] **Step 7: Run the full test suite**

Run: `cargo test`
Expected: PASS (workspace-wide: core + daemon unit + telnet_e2e + api_e2e + mcp_bridge + persistence tests).

- [ ] **Step 8: Commit**

```bash
cargo fmt
git add crates/snakewood-daemon/src/api/ crates/snakewood-daemon/src/engine.rs crates/snakewood-daemon/tests/api_e2e.rs crates/snakewood-daemon/tests/mcp_bridge.rs
git commit -m "feat(daemon): JSON API deferred-response drain; retire Engine::submit"
```

---

## Stage 4 — Seed proof operators + hardening

**Goal:** The running daemon ships the proof operators in `world/operators.ron`; document the tick-as-quantum and Coalesce hazard; final verification.

### Task 4.1: Seed proof operators into the daemon world

**Files:**
- Modify: `crates/snakewood-daemon/src/main.rs`

**Interfaces:**
- Consumes: `Realm.operators`, `Engine::checkpoint`, `snakewood_core::{Operator, IntentClass, PresentationKind, Scope}`.

- [ ] **Step 1: Add operator seeding**

In `crates/snakewood-daemon/src/main.rs`:
- Extend the core import to include the operator types:

```rust
use snakewood_core::{
    Direction, EntityId, GitStore, IntentClass, Operator, PresentationKind, Room, Scope,
};
```

- Add a seeding function below `seed_if_empty`:

```rust
/// Attach the M2 proof operators if none are configured: rate-limit movement
/// (one step per 4 ticks per actor) and coalesce redundant room redraws.
fn seed_operators_if_empty(engine: &mut Engine) -> Result<(), Box<dyn std::error::Error>> {
    if !engine.realm().operators.is_empty() {
        return Ok(());
    }
    engine.realm_mut().operators = vec![
        Operator::RateLimit {
            on: IntentClass::Move,
            per_ticks: 4,
            scope: Scope::PerActor,
            deny: Some("You catch your breath before moving again.".to_string()),
        },
        Operator::Coalesce {
            on: vec![
                PresentationKind::RoomName,
                PresentationKind::RoomDescription,
                PresentationKind::Exits,
                PresentationKind::Occupants,
            ],
            within_ticks: 1,
            scope: Scope::PerActor,
        },
    ];
    engine
        .checkpoint("seed M2 operators")
        .map_err(|err| format!("{err:?}"))?;
    Ok(())
}
```

- Call it in `main` right after `seed_if_empty(&mut engine)?;`:

```rust
    seed_operators_if_empty(&mut engine)?;
```

- [ ] **Step 2: Verify the daemon boots and persists operators**

Run (writes to a throwaway data dir, boots for 1s, then exits):

```bash
SNAKEWOOD_DATA=/private/tmp/claude-502/-Users-nathan-Projects-snakewood/fb1a5eb9-51b1-4f64-959d-915befc6011a/scratchpad/m2boot \
SNAKEWOOD_ADDR=127.0.0.1:4500 SNAKEWOOD_API_ADDR=127.0.0.1:4501 \
timeout 1 cargo run -p snakewood-daemon; \
cat /private/tmp/claude-502/-Users-nathan-Projects-snakewood/fb1a5eb9-51b1-4f64-959d-915befc6011a/scratchpad/m2boot/world/operators.ron
```

Expected: exit after 1s; `operators.ron` exists and contains `RateLimit(` and `Coalesce(`.

- [ ] **Step 3: Commit**

```bash
cargo fmt
git add crates/snakewood-daemon/src/main.rs
git commit -m "feat(daemon): seed M2 proof operators (RateLimit(Move), Coalesce redraws)"
```

---

### Task 4.2: Final verification

**Files:** none (verification only).

- [ ] **Step 1: Format check**

Run: `cargo fmt --check`
Expected: no output (clean). If it reports diffs, run `cargo fmt` and commit.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings. Fix any and commit with message `chore: clippy clean for M2 operators`.

- [ ] **Step 3: Full test suite**

Run: `cargo test`
Expected: all tests PASS.

- [ ] **Step 4: Confirm success criteria (spec §8)**

Map each to a green test (no new code — just verify presence):
1. Operator round-trip: `operator::tests::any_operator_list_round_trips` + `store::git::tests::operators_round_trip_through_git`.
2. RateLimit admits/drops with deny: `engine::tests::drain_rate_limits_moves_across_ticks` + `operator::tests::rate_limit_admits_one_per_window_then_drops`.
3. Coalesced single room view: `engine::tests::drain_coalesces_repeated_room_views_in_one_tick`.
4. Determinism: `RateLimiterState` + `coalesce` are pure and driven by `tick`; `ManualClock` tests are reproducible.
5. Live post-drain delivery: `telnet_e2e`, `api_e2e`, `mcp_bridge` all go through enqueue → drain → poll.

- [ ] **Step 5: Update the project status memory**

Append an M2-operators entry to `/Users/nathan/.claude/projects/-Users-nathan-Projects-snakewood/memory/snakewood-status.md` recording: execution model B landed (tick-drained queue), operators persisted to `world/operators.ron`, `Engine::submit` retired in favor of `enqueue`+`tick`, heartbeat is `SNAKEWOOD_TICK_MS` (default 250ms), and remaining M2 sub-projects (presentation vocab, SSH/WS gateways). No commit needed (memory dir is outside the repo).

---

## Self-Review Notes (for the implementer)

- **Coalesce `within_ticks` is stored but only within-batch collapse is implemented** — this is intentional (spec §5). Do not add cross-tick redraw suppression.
- **`RateLimit` on `Look` is not seeded** — the connect/greeting Look must never be rate-limited, or a fresh connection would get no initial view.
- **Reader-task lifetime (telnet):** the cancel-safe reader task may outlive a `quit` until the socket closes; this is acceptable for M2. If you observe leaked tasks in a soak test, that's a future hardening item, not an M2 blocker.
- **Carry-forwards NOT in scope here** (leave for later M2 sub-projects / backlog): accept-loop backoff, `TcpDaemonClient::set_read_timeout`, session-only `Disconnect` for named actors, `StoreError: Display`, scheduler prune trap. Do not fold them in.
