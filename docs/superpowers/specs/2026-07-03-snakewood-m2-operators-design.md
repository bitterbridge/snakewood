# Snakewood M2 — Operators — Design

**Status:** Approved (design). Ready for implementation planning.
**Date:** 2026-07-03
**Parent milestone:** M2 (spec §8). This is the first of three M2 sub-projects (Operators · richer presentation vocabulary · SSH/WS gateways), each its own spec → plan → implementation cycle.
**One-line:** Realize the event fabric's operator vocabulary (`RateLimit`, `Coalesce`) by moving the daemon from synchronous per-`submit` dispatch to a deterministic, tick-drained intent queue — the FRP model the design has always pointed at.

---

## 1. Goal & Motivation

The design spec (`2026-07-02-snakewood-mud-design.md`) §4.6 describes **operators**: a curated, growing vocabulary of deterministic stream state machines — `RateLimit`, `Coalesce`, `Debounce`, `Window`, `Sample` — attached declaratively at stream seams, defined in **tick units** against the injected `Clock`. M1 explicitly deferred them.

Building them now forces a decision M1 left open: M1's `Engine::submit` **dispatches synchronously and immediately**, with no intent queue and no tick-quantized processing. Operators are inherently about buffering and gating across ticks, so they cannot sit on top of immediate dispatch. This sub-project therefore delivers the operator vocabulary **and** the execution model that makes it — and M3's Notify-driven mob reactions — possible.

**Goal:** build the generic operator machinery (vocabulary + attachment-as-data + a tick-quantized evaluation harness) and prove it end-to-end against the intents that exist today (`Move`, `Look`). The canonical combat consumer (`RateLimit(Attack, …)`) arrives in M3 with **zero new operator machinery** — just a new intent class.

**Non-goals (deferred):** `Debounce`/`Window`/`Sample` operators; `PerSession` scope; Notify broadcast to bystanders (M3); combat/`Attack`; cross-tick coalescing of room redraws (see §5).

## 2. Chosen Execution Model — B: tick-drained intent queue

Considered three models (recorded for context):

- **A — Inline gates:** keep synchronous dispatch; RateLimit as an admission check at `submit`; Coalesce buffers the outbox. Smallest change, preserves instant commands, but is not the eventual architecture and gives Coalesce/M3-reactions no real home.
- **B — Per-tick intent queue (CHOSEN):** `submit` enqueues; `tick()` drains the queue through operators → dispatch → event-operators → flush. The honest FRP realization; the natural home for M3's "NPC observes entry → next tick emits Attack." Command latency becomes one tick.
- **C — Hybrid:** un-operated intents dispatch immediately, operated ones queue. Most flexible, most code.

**B is chosen** because it is the eventual destination and M3 needs it regardless; paying the cost once, now, is cheaper than A-then-B.

### 2.1 The drain

The **tick is the single quantum** — both game cadence and command-processing beat. Every tick:

1. Snapshot the intent queue and clear it.
2. For each `(SessionId, Intent)` in **FIFO enqueue order**: evaluate against attached **intent-operators** (`RateLimit`). If admitted, `dispatch(realm, intent)` and collect its `Dispatch` (events + directed presentation). If dropped, emit the operator's optional deny node to the actor's sessions.
3. After the whole batch is dispatched, pass the collected directed-presentation through **event-operators** (`Coalesce`), per recipient, then flush the survivors to session outboxes.

Determinism: the outcome is a pure function of `(tick number, the ordered set of intents enqueued before that tick)`. No wall-clock enters the logic; operator windows are counted in ticks. Tests drive `tick()` directly via `ManualClock`.

`Engine::submit` remains the authorization seam (a session may only enqueue intents for the actor it is bound to); authorization is checked **at enqueue time**. Dispatch still proceeds even if the submitting session has since disconnected (the world mutation is legitimate; undeliverable presentation is simply discarded on `poll`).

**`dig` is unaffected.** It is OOC world-building (`Engine::dig`: direct mutate + `checkpoint`), never an intent, so builder actions stay synchronous and immediate. Only IC intents (`Move`, `Look`) traverse the queue. `Look` is queued uniformly with `Move` — one code path; the ≤250 ms latency is imperceptible and read-fast-pathing is a future optimization if ever needed.

## 3. Operator Vocabulary (core, data, serde/RON)

New types in `snakewood-core::fabric`:

```rust
enum IntentClass { Move, Look }                 // grows alongside the Intent enum
enum PresentationKind {                         // maps to PresentationNode variants
    RoomName, RoomDescription, Exits, Occupants, Line, Denied,
}
enum Scope { Global, PerActor }                 // PerSession deferred to a later milestone

enum Operator {
    RateLimit { on: IntentClass, per_ticks: u64, scope: Scope, deny: Option<String> },
    Coalesce  { on: Vec<PresentationKind>, within_ticks: u64, scope: Scope },
}
```

- **`RateLimit`** admits a matching intent only when `tick >= last_admitted + per_ticks` for its scope key (and always on the first admission), driven by the tick counter — a sliding window from the last admission, not a fixed tumbling window. `Global` = one bucket for all actors; `PerActor` = a bucket per actor id. Excess intents are **dropped**; the actor receives `PresentationNode::Denied(deny)` when `deny` is `Some`, or a generic default message when `None`. (M2 always surfaces a deny; a truly silent mode is out of scope.) *Rationale:* dropping (vs. deferring) matches movement intuition — spamming a direction must not stack laggy queued moves that fire seconds later. Defer-style behavior is a future `Debounce`.
- **`Coalesce`** collapses redundant directed-presentation nodes. In M2 it operates **within a single tick's batch**, per recipient: for each configured `PresentationKind`, keep the last-emitted node of that kind, discard earlier ones. So several `Move`s draining in one tick yield one clean view of the *final* room, not N stacked redraws. `within_ticks` is retained in the vocabulary and the state machine generalizes, but M2 proves and ships only `within: 1` (see §5 hazard).

### 3.1 Where operators live and persist

Operators are **authored world data** (design constraint: everything is data, round-tripping to git). They live on `Realm.operators: Vec<Operator>` alongside `rules`, and persist through `WorldStore` to **`world/operators.ron`**, parallel to `world/rules.ron` — loaded in `load_realm`, saved in `save_realm`. Editing operators is deliberate authoring (`on-change`/`on-checkpoint` cadence, same as rules).

### 3.2 Definitions vs. runtime state

The `Operator` **definitions** are pure, persisted core data. The **runtime state machines** (per-key last-admitted tick for RateLimit; per-batch collapse for Coalesce) are runtime-only and live in the Engine (like sessions — never persisted). Core exposes pure evaluation helpers (e.g. `RateLimitState::admit(op, key, tick) -> bool`, a `coalesce(nodes, ops) -> nodes` function) so the operator logic is unit-tested at the cheapest layer without a daemon.

## 4. Daemon changes

### 4.1 Engine

- `intent_queue: Vec<(SessionId, Intent)>` (FIFO).
- Operator runtime state, seeded from `realm.operators`.
- `submit(session, intent)`: authorize, then **enqueue** (no dispatch).
- `tick()`: increment the tick counter, then run the §2.1 drain (RateLimit → dispatch → Coalesce → flush).
- `sessions_with_pending() -> Vec<SessionId>`: sessions whose outbox is non-empty, so the delivery path flushes only those (closes the M1 backlog item; avoids polling every session every heartbeat).
- `poll` unchanged (drains a session's outbox).

### 4.2 Heartbeat & snapshot decoupling

- `SNAKEWOOD_TICK_MS` env var, **default 250 ms**, drives the drain heartbeat in the tokio tick loop.
- `maybe_snapshot` stays gated by its own wall-clock second-interval, unaffected by the faster heartbeat.
- `SystemClock` in production; `ManualClock` in tests drives ticks explicitly.

### 4.3 Transport delivery restructure (the real cost of B)

Because output now appears only after a drain, the M1 inline `submit → poll → write` flow no longer works:

- **Telnet** (`telnet/server.rs`): per-connection loop becomes a `tokio::select!` between (a) socket-readable → read line → parse → `submit` (enqueue), and (b) a per-connection heartbeat → `poll` outbox → render → write. Delivery latency is bounded by one heartbeat.
- **JSON API** (`api/server.rs`, `api/handler.rs`): a request that enqueues an intent (`Move`, `Look`) must return that intent's result. The handler enqueues, `await`s a **drain-complete signal** (a shared `tokio::Notify` fired by the tick loop after each drain), then `poll`s the session and returns the rendered presentation. The request/response contract is preserved; latency ≤ one heartbeat. `Connect`/`Dig`/`Disconnect` remain synchronous (they are not intents).
- **MCP bridge** (`snakewood-mcp`): no change — it speaks the JSON API, which still answers each request with the resulting observation.

## 5. Known hazard, documented

Cross-tick coalescing of **room redraws** is unsafe: suppressing a `RoomName` at tick *t*+1 because one was flushed at tick *t* would hide a room the player actually moved into. M2 therefore ships and tests only `within: 1` (collapse within a single tick's batch), where every node in the batch describes the same end-of-tick state. `within_ticks > 1` remains expressible for future kinds where cross-tick suppression is meaningful (e.g. status-line spam), but must not be applied to navigational redraws. This constraint is called out in code comments on the Coalesce evaluator.

## 6. Testing Strategy

Cheapest layer first, per the design's testing doctrine:

- **Core unit (the bulk):**
  - `RateLimit` admits exactly one matching intent per `per_ticks` window; independent buckets per `PerActor` key; one shared bucket for `Global`; deny text surfaces.
  - `Coalesce` collapses a batch to last-wins per configured kind; leaves non-configured kinds (e.g. `Denied`, `Line`) untouched and ordered.
  - `Operator` RON round-trips (proptest over generated operator lists + a golden-file snapshot), and `world/operators.ron` survives a `WorldStore` save → reload → identical.
- **Engine integration (in-process, no socket):**
  - Enqueue N `Move`s toward an open exit; advance ticks; assert exactly the rate-limited number commit and the final position is correct.
  - Enqueue several `Move`s that all drain in one tick; assert the actor's outbox contains a single coalesced room view of the final room.
  - Deny node reaches the actor when a `Move` is rate-limited.
- **End-to-end (few):**
  - Real telnet: spam a direction; observe the deny message and bounded movement cadence; normal walking still works.
  - Real JSON API: a `Move` request returns the post-drain observation (proving the enqueue → drain-signal → poll → respond path).

## 7. Implementation Staging (indicative — firmed up in the plan)

Roughly four stages, each compiling and green:

1. **Operator vocabulary + persistence (core):** `Operator`/`Scope`/`IntentClass`/`PresentationKind` types, `Realm.operators`, `world/operators.ron` load/save, RON round-trip + golden tests. Pure evaluation helpers (`RateLimit` admit, `coalesce`) with unit tests. No daemon wiring yet.
2. **Engine intent queue + drain:** `submit` enqueues; `tick()` drains (RateLimit → dispatch → Coalesce → flush); `sessions_with_pending`. Engine integration tests with `ManualClock`. Transports still work via the new heartbeat delivery (stage 3) — until then, integration tests drive `tick()` directly.
3. **Heartbeat + transport delivery restructure:** `SNAKEWOOD_TICK_MS`; telnet `select!` reader/writer; API enqueue → drain-signal → poll → respond. E2E telnet + API tests.
4. **Hardening & carry-forwards:** wire the proof operators into the seed world's `operators.ron`; docs/comments (Coalesce hazard, tick-as-quantum); confirm `maybe_snapshot` decoupling. Optional pickup if cheap: accept-loop backoff (M1 carry-forward) since we are already in the transport loops.

## 8. Success Criteria (all testable)

1. An operator list round-trips: `Realm` with operators → `world/operators.ron` → reload → identical (proptest + golden green).
2. With `RateLimit(Move, per_ticks: N, PerActor)` attached, submitting M > (ticks/N) moves over a tick span commits exactly the admitted number; the rest are dropped and the actor receives the deny node.
3. Several `Move`s draining in one tick produce exactly one coalesced room view of the final room in the actor's outbox.
4. Determinism: the same enqueue-then-tick sequence under `ManualClock` yields identical committed state and presentation across runs.
5. Live: telnet and the JSON API both deliver post-drain output within one heartbeat; `dig` and other OOC calls remain synchronous.
