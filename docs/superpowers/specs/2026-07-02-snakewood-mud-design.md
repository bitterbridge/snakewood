# Snakewood — Design

**Status:** Approved (design). Ready for implementation planning.
**Date:** 2026-07-02
**One-line:** An experimental, data-driven MUD in Rust whose world is a version-controlled, live-mutable object graph, driven by a reactive event fabric, and steerable by a first-class MCP.

---

## 1. Vision & Guiding Constraints

Snakewood is a classic MUD (rooms, mobs, items, exploration) built around one distinctive ambition: **you expand and reshape the world from within, and every change round-trips cleanly to a version-controlled data store.** It draws on the LPMud/Nightmare lineage (a live, creator-extensible world) but disciplines it with DevOps practice: the world is data, diffable, reviewable, and its history is the world's memory.

Non-negotiable constraints that shape every decision below:

1. **Everything is data. No embedded scripting.** Behavior comes from a fixed, *growing* vocabulary of engine-implemented primitives, composed declaratively. Adding a genuinely new capability means adding a Rust primitive; composing existing ones is pure data.
2. **Mutate-first, with clean round-tripping as a principal concern.** You change the world by acting on the live world; the live world serializes back out faithfully. The in-memory model and the on-disk representation are two views of the same thing. The serializer is deterministic and canonical so diffs are meaningful.
3. **Everything can round-trip to git, governed by per-type commit cadence.** Not a binary "in git vs. not," but a spectrum of commit triggers. Git history becomes the world's event log.
4. **The MCP is a first-class citizen.** Development happens in partnership with an AI agent that acts in the world both IC (as a character) and OOC (as a builder/admin). The architecture must let the MCP restart frequently without disturbing the world.
5. **Best-practices architecture, testable at the cheapest layer.** Pure core library, thin host, thin transports; injected clock and RNG; storage behind a trait.

A recurring design motif: **several small, growing, declarative vocabularies** — behavior *primitives*, *predicates*, *effects*, *presentation elements*, *persistence policies*, and stream *operators*. Each starts minimal and grows agile-style. A fixed Rust evaluator interprets each; authors only ever compose data.

---

## 2. System Architecture

Three layers, with a hard rule: **business logic lives only in the bottom layer.**

```
TRANSPORTS = RENDERERS (thin, swappable, individually restartable)
  telnet gateway  — presentation tree -> ANSI / MXP        (canonical human front door)
  ssh gateway     — presentation tree -> ANSI (over ssh)
  ws gateway      — presentation tree -> HTML/CSS
  MCP server      — presentation tree AS structure (IC) + structured world API (OOC), reconnecting
        |  typed presentation tree in-process; serialized (XML-ish) on the wire
        v
snakewood-daemon (HOST)
  owns: running world · tick loop · session registry · API binding to a local socket
        · persistence scheduler · injected Clock + seeded Rng
        |  direct function calls (no socket)
        v
snakewood-core (LIBRARY — pure, deterministic, no I/O)
  world model · event fabric · behavior/predicate/effect vocabularies
  · canonical serializer · persistence policies
        |  WorldStore trait
        v
  git-backed world repo (world/)  +  live-state store (state/)
```

**Load-bearing decisions:**

- **`snakewood-core` is a pure library.** The entire world model, the event fabric, every vocabulary, the serializer, and the persistence-policy engine. No sockets, no clock, no filesystem reached directly. Nearly all logic and nearly all tests live here.
- **The daemon is a thin, long-lived host.** It owns the running world and the tick loop, injects a `Clock` and seeded `Rng`, runs the persistence scheduler, and binds the core's API to a stable local socket. Because it is long-lived, restarting *any* transport — including the MCP when Claude Code restarts — never disturbs the world.
- **Transports are thin renderers.** The core emits a **semantic presentation model**; each transport renders it to its medium. Telnet is the human lowest-common-denominator; SSH/WS are gateways that terminate their protocol and render the same presentation stream. The MCP is a **reconnecting thin client** that consumes the presentation tree *as structure* (IC) and calls the structured world API (OOC) — it never scrapes telnet text.
- **Two storage targets behind a trait.** The version-controlled world (`world/`) and the live-state store (`state/`) are both reached through a `WorldStore`-style trait, so tests swap in temp/in-memory implementations.

**Crate layout (Cargo workspace):** `snakewood-core` (lib), `snakewood-daemon` (bin), `snakewood-telnet`, `snakewood-mcp`, with `snakewood-ssh` / `snakewood-ws` gateways later. Crate boundaries enforce the layering: a transport crate cannot reach core logic except through the API.

### 2.1 The Semantic Presentation Model

The core never emits ANSI or medium-specific bytes. It emits **meaning**: "this is a room description," "this is an exit named *north*," "this span is *danger*," "this is player speech," "this is a prompt." Internally this is a **typed presentation tree** (an enum of message kinds with styled spans carrying *semantic roles*). On the wire (for out-of-process gateways) it serializes to an XML-ish / MXP-like form. At the edge, each transport renders it.

Consequence and built-in cross-check: **the MCP and a human telnet player receive the same semantic events**, only rendered differently. What the MCP builds is provably the same content a player sees.

The core's output splits in two: a **semantic presentation stream** (narrative, rendered per-client) and **structured observations** (typed world-state for queries/OOC — MCP uses raw, telnet renders a subset).

---

## 3. The World Model & Data

The model is deliberately **boring and typed**: domain records + a behavior/handler enum, not a general ECS.

### 3.1 Entity kinds

- **Rooms** — singleton authored structure; one definition = one place. Hold exits (references) and handlers.
- **Prototypes** — the authored template for a **mob** or **item** (LPMud "blueprint"). Spawned, never walk around themselves.
- **Instances** — a live spawned mob/item created from a prototype; carries only deviations from its proto (current HP, position, contents) plus a back-reference.
- **Zones** — a directory-sized grouping: metadata + spawn rules + the rooms/protos that belong to it. The unit of authoring and (often) ownership.
- **Characters** — persistent player instances (account-linked); structurally an instance with extra durability.

### 3.2 Anatomy of an entity (all data)

1. **Stable ID** — human-readable, namespaced, *not* a UUID, so diffs and references are legible: `snakewood/clearing`, `snakewood/mob/orc`. Instances get `proto-id#serial`, e.g. `snakewood/mob/orc#42`.
2. **Attributes** — plain typed fields (name, descriptions, stats, weight…).
3. **References** — always by ID (a room's `exits`, an inventory as a list of instance IDs). No inline nesting of other entities — keeps files small and diffs surgical.
4. **Handlers** — see the event fabric (§4). The common case is sugar; the general case is data responders.

### 3.3 On-disk format & layout

**RON, one file per authored entity, grouped by zone.** RON maps losslessly onto Rust enums-with-fields (exactly what handlers/primitives are) and reads cleanly.

```
world/
  rules/                    # global rules (first-class, unattached handlers)
    movement.ron
    combat.ron
  snakewood/
    zone.ron                # metadata, spawn rules, ownership
    rooms/clearing.ron
    rooms/old-well.ron
    mobs/goblin.ron         # prototype
    items/rusty-sword.ron   # prototype
state/                      # live instances & characters (different commit cadence, §5)
```

**The serializer is canonical** — stable field order, sorted maps, normalized whitespace — so a mutation produces a *minimal, meaningful* diff. One-file-per-entity means digging a room is one new file and merges stay sane. This is the round-trip machinery the persistence subsystem rides on.

**Prototype vs. instance is also the git story:** editing a prototype (`world/`) is deliberate authoring and commits eagerly; a spawned instance (`state/`) churns and commits on cadence.

---

## 4. The Event Fabric (the engine)

An OO/FRP synthesis: an object graph where reality flows as signals. This subsumes commands, exits, event-hooks, and mob reactions into one model. **The fabric is the wiring; handlers remain data** (from the fixed predicate/effect/presentation vocabularies). No scripting.

### 4.1 Intent vs. Event

- **Intent** — a proposed, **vetoable/transformable** signal: *"Actor wants to Move(North)."*
- **Event** — a **committed, factual, observable** signal: *"Actor entered Old Well."*

### 4.2 Three-phase lifecycle

1. **Guard** — candidate handlers may `Deny(reason)`, `Redirect`, or `Allow`. No committed state change yet.
2. **Commit** — if not denied, the state change applies, producing Events.
3. **Notify** — Events publish to observers, who **cannot veto** but may **emit new Intents** (e.g., an NPC observes "PC entered" and next tick emits an Attack intent).

Mapping: "no exit" / "What?" / locked / goblin-block are **Guard** vetoes; NPC aggression is a **Notify** reaction. This split keeps the model deterministic and rollback-free: deciders run in Guard, reactions run in Notify.

### 4.3 Guard is two passes

**Pass 1 — Resolve the outcome (unordered, set-based).** Every candidate handler votes `Allow` / `Deny` / `Redirect` / `Abstain`. Outcome is a pure function of the *set* of votes with the lattice **`Deny` > `Redirect` > `Allow`**. Any `Deny` present ⇒ denied. An `Allow` can never override a `Deny`, so **invariants are unbypassable** (e.g., "locked doors impassable" is a `Deny` no nearer handler can out-vote). Outcome does not depend on evaluation order.

**Pass 2 — Select narration & reaction (ordered by salience).** Among the deniers, pick the **most salient** to (a) narrate and (b) fire its reaction effect. Salience default: band order **`Participant → Structure → Global`** plus an explicit **priority integer** override. So a blocking goblin out-narrates a locked door — you hear about the goblin; kill it and try again, and you then hear about the lock. Reaction effects: by default only the salient denier reacts; a `always_reacts` flag (deferred until needed) lets exceptions like a tripwire alarm always fire.

Because outcome is set-based, band order affects only *storytelling*, not correctness — getting salience "wrong" yields a less apt message, never a wrong outcome.

### 4.4 Derived + guarded subscription

Subscriptions are **not** hand-maintained lists (which rot on move/death/teleport/destroy). Instead:

- **Candidate set is derived from structure** — co-present entities, the room, the relevant exit/door, plus all **global rules**. Co-presence *is* the subscription, recomputed, never stored.
- **Participation is gated by predicates** on current state.

This dissolves the dead/unconscious-goblin problem for free: the goblin's block handler is guarded `require: [Alive(Self), Conscious(Self)]`. Dead ⇒ predicate false ⇒ doesn't block; revived ⇒ blocks again. No subscribe/unsubscribe churn — the guard *is* the dynamic subscription.

### 4.5 Handlers: Rules and Responders

- **Rules** — global, unattached handlers living in `world/rules/`, subscribing to intent *classes* (e.g., "locked doors impassable," "no exit → message"). Written once; individual entities don't repeat the logic.
- **Responders** — entity-attached handlers. Data shape:
  ```
  Responder(
      on:      <message pattern>,   // Move(North), Look, Open("gate"), Attacked, Tick(30)…
      require: [<predicate>, …],    // guards, from the Predicate vocabulary
      do:      [<effect>, …],       // effects, from the Primitive/Effect vocabulary
      result:  <outcome>,           // Traverse(room) | Stay | Redirect(...) | Consume | …
  )
  ```
- **Common case is sugar.** `exits: { north: "snakewood/old-well" }` and plain text descriptions desugar to trivial handlers. Authors only reach for full responders when physics gets interesting (doors, guards, curtains of light).

Worked example (ordered responders express ZIL/Inform-style branching; note that outcome is still set-based, salience picks the message):

```ron
Room(
    id: "snakewood/clearing",
    name: "Snakewood Clearing",
    responders: [
        ( on: Move(West),
          require: [ Locked("snakewood/gate"), Holds(Actor, "snakewood/item/copper-key") ],
          do:      [ Unlock("snakewood/gate"),
                     Narrate(Actor, "You unlock the door with a copper key and proceed west.") ],
          result:  Traverse("snakewood/gate-house") ),
        ( on: Move(West),
          require: [ Locked("snakewood/gate") ],
          do:      [ Narrate(Actor, "You bump your head into the closed, locked door.") ],
          result:  Stay ),
        ( on: Move(North), result: Traverse("snakewood/old-well") ),  // plain open exit
    ],
)
```

### 4.6 Operators

A curated, **growing vocabulary** of stream operators — `RateLimit`, `Coalesce`, `Debounce`, `Window`, `Sample` — attached **declaratively** at stream seams (on an Intent stream before dispatch; on an Event stream before Notify). **Not** free-form combinator composition (which would wreck determinism and inspectability); each operator is a small deterministic state machine you attach as data.

**The tick is the quantum of determinism.** Every operator is defined in **tick units** against the **injected Clock**, never wall-clock — so operators are reproducible and testable with a virtual clock ("advance 3 ticks → exactly one attack landed"). Examples: `RateLimit(Attack, 1 per 3 ticks, per: Actor)` (attack-round cadence); `Coalesce(RoomChanged, within: 1 tick)` (no redraw spam); `Debounce(Move, 1 tick)`. Scopable global / per-entity / per-session.

### 4.7 Transports as sources & sinks

Every transport is just an **Intent source + a presentation-Event sink** on the same fabric. Telnet, SSH, WS, and the MCP are peers feeding and draining one bus. That is *why* the transport layer is symmetric. Unknown verbs fail at the **parser** ("What?") before an intent forms; structurally-invalid actions fall through to low-priority **default rules** ("You see no exit in that direction").

### 4.8 Traceability

Every intent produces a **traceable log** of the handlers it touched and their votes. This trace is (1) how tests assert behavior (assert the trace, not ANSI), (2) how the MCP answers "why did that happen?" OOC, and (3) the feed for §5's committable events.

---

## 5. Persistence & the Living World

Reframe: not "in git vs. not," but **commit cadence per kind of state.** Everything *can* round-trip to git; what differs is *what triggers the flush*. Each schema type **declares a persistence policy**; a **commit scheduler** coalesces pending mutations and writes them with meaningful messages. **Git history becomes the world's memory and event log** (`"Nathan dug Snakewood Clearing"`, `"hourly world snapshot"`, `"Grimlock died permanently"`).

**Two axes:**

- *Data nature*: authored structure · slow-living state · fast/volatile state · truly ephemeral (GC'd, never persisted).
- *Commit trigger*: `on-change` · `on-interval` · `on-checkpoint` · `on-event` · `never`.

**Starter policy set (grow the taxonomy agile-style):**

- **`on-change`** — authored structure (rooms, prototypes, rules). A deliberate build (e.g., `dig`) commits immediately.
- **`on-interval`** — best-effort/idle snapshots (e.g., hourly), driven by the injected clock.
- **`on-checkpoint`** — manual save and automatic pre-reboot save.

Live state (player position, spawned instances) starts on `on-interval` / `on-checkpoint`. `on-event` (committable moments like permadeath) and a fuller taxonomy come later. Significant Events on the fabric (§4.8) are exactly what an `on-event` policy subscribes to.

---

## 6. Testing Strategy

The layering exists so we can test at the **cheapest layer that covers the behavior**; the socket boundary is *not* where logic gets tested. Requirements baked in from Stage 1: **injected `Clock`**, **seeded `Rng`**, and **git behind the `WorldStore` trait**.

- **Core unit tests (the bulk)** — world mutations, the fabric, vocabularies, persistence-policy engine. In-process, fast, deterministic.
- **Round-trip / property tests (the principal concern, tested cheapest)** — `mutate → serialize → deserialize → assert identical` as proptest over generated worlds, plus **golden-file snapshots** of serialized output; and the full `serialize → commit to a temp git repo → reload → identical`.
- **API integration tests** — drive commands against an in-process daemon (no socket); assert the structured observation *and* the resulting mutation *and* the persistence effect. The MCP and the test harness call the *same* structured API.
- **Transport tests** — telnet codec (bytes ↔ commands/events) and gateways as pure translators.
- **End-to-end (few)** — boot a real daemon, connect real telnet + real MCP, run a scenario.
- **Bonus** — **replay tests** over a commit sequence (git *is* the event log); **soak tests** over N simulated ticks asserting invariants (no leaked instances, GC reclaims, bounded populations).

---

## 7. First Milestone — the Walking Skeleton

**Goal: prove the spine end-to-end and de-risk the round-trip — the thinnest slice that touches every seam.** A correct, observable MUD, not yet a fun one.

### In scope (one thin vertical slice)

- **Workspace**: `snakewood-core` · `snakewood-daemon` · `snakewood-telnet` · `snakewood-mcp`. **Injected `Clock` + seeded `Rng` from commit one.**
- **World**: `Room` with sugar-exits + responders; ~3 hand-authored rooms in RON; the **canonical deterministic serializer**; `WorldStore` trait with a **git-backed** impl and a **temp/in-memory** impl. A minimal **goblin prototype + instance** (the M1 guarded exit; blocks, does not fight).
- **Fabric**: `Intent`/`Event` types; **one intent — `Move(dir)`** — plus `Look`; full Guard two-pass; a global **"no exit" fallback Rule**; a **blocking goblin** exercising predicate-gated subscription (`Alive`/`Conscious`) and salient narration; Commit (relocate actor); Notify (emit "entered" → presentation). The **tick loop** runs (daemon liveness). **Operators deferred to M2.**
- **Presentation**: minimal node set — `RoomDescription`, `ExitList`, `Narrate`, `Deny`, `Prompt`. Telnet renders → ANSI; MCP consumes as structure.
- **Transports**: telnet listener (connect anonymously, `look`, `n/s/e/w`, see rendered output); **MCP thin client** — IC (`look`, `move`) returning structured observations + OOC (**one mutation: `dig <dir> <name>`**), reconnecting.
- **Headline proof**: MCP OOC `dig north "Old Well"` → mutates live world → serializer writes RON → `WorldStore` commits to git → `reload` re-reads → **world identical**.
- **Persistence**: the three starter policies (`on-change`, `on-interval`, `on-checkpoint`). Live state (player position) → interval/checkpoint.

### Success criteria (all testable)

1. Telnet: connect, `look`, walk between rooms; a wall gives *"no exit"*; the goblin denies with the **correct salient message**; kill/remove the goblin and the exit opens.
2. MCP IC: `look`/`move` return **structured** observations (asserted structurally, not scraped).
3. MCP OOC: `dig` creates a room → RON file appears → git commit recorded → **reload yields an identical world** (proptest round-trip + golden-file snapshot both green).
4. **Restart the MCP** (simulating a Claude Code restart) → it reconnects → world + session state intact.
5. Injected clock: advancing one simulated hour produces exactly one `on-interval` snapshot commit.

### Explicitly deferred (YAGNI)

SSH/WS gateways · operators · combat · mob AI/aggression · loot · accounts/auth depth · participant responder-chains at scale · `always_reacts` · in-game builder commands beyond `dig` · the full persistence-policy taxonomy.

### Why this slice

It forces all three layers, both output fidelities, all three Guard phases, the git round-trip, the injected clock/RNG, the `WorldStore` trait, and the prototype→instance + `state/` lane to exist and interlock — so the architecture is *proven*, not just drawn. Everything after is content and vocabulary growth on a trusted spine.

---

## 8. Roadmap (indicative, post-M1)

Each is its own spec → plan → implementation cycle.

- **M2** — Operators (`RateLimit`, `Coalesce`); the SSH and WS gateways; richer presentation vocabulary.
- **M3** — Combat & mob AI (aggression via Notify reactions); loot tables; the `on-event` persistence policy (permadeath, zone completion).
- **M4** — Accounts/auth; characters as durable instances; player persistence hardening.
- **M5** — In-game builder command suite (beyond `dig`); ownership/permissions for zones; the fuller persistence-policy taxonomy.
- **Ongoing** — grow the vocabularies (primitives, predicates, presentation, operators, policies) as content demands.
