# M2 Richer Presentation Vocabulary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce the deferred semantic styled-span model — free-text presentation carries `Span`s tagged with a `Role` — so one core output renders to telnet ANSI, is consumable as structure by MCP/API, and is ready for a future WS gateway.

**Architecture:** `snakewood-core` gains a `Role` enum and a `Span { text, role }` type; the four free-text `PresentationNode` variants (`RoomDescription`, `Occupants`, `Line`, `Denied`) change their payloads to `Vec<Span>` while the coarse-role variants (`RoomName`, `Exits`, `Prompt`) stay as-is. The core auto-tags occupant names with `Role::Actor`; all other text is a single `Role::Default` span. The telnet renderer gains an `Ansi`/`Plain` mode (role→SGR + variant→base-style tables); MCP/API consume the same structured stream.

**Tech Stack:** Rust, `serde`/`ron`, current-thread `tokio`, `proptest`.

## Global Constraints

- **Do NOT rename or reorder `PresentationNode` variants.** Operators (`PresentationKind::of`, `coalesce`, `RateLimit`) match by variant discriminant; only payload *types* may change. Operators must keep compiling and passing with zero edits.
- **Minimal role vocabulary:** `Role { Default, Actor }` only. The core emits `Actor` solely on `Occupants` entries; all other free text is a single `Default` span. Do NOT add other roles or inline-role markup for authored text (deferred).
- **`Plain` telnet output must be byte-identical to M1** (regression guard): same line formatting (`"Exits: …"`, `"Also here: …"`, omit empty occupants, `"Exits: none"`, `">"` prompt), CRLF line endings.
- **No changes to the event fabric, operators, or the engine tick/drain logic.** (The one engine edit is mechanical: the rate-limit `Denied(text)` construction becomes `Denied(plain_text(text))`.)
- **Every commit compiles, passes all existing tests, and is `cargo fmt`-clean.** Never use `--no-verify`.
- **Canonical RON/serde:** `Span`/`Role` derive the same set as the existing nodes (`Serialize, Deserialize, Debug, Clone, PartialEq`; `Role` also `Copy, Eq`).

---

## Task 1: Add `Role`, `Span`, and helpers (additive, core)

**Files:**
- Modify: `crates/snakewood-core/src/presentation.rs`
- Modify: `crates/snakewood-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `enum Role { Default, Actor }`
  - `struct Span { pub text: String, pub role: Role }` with `Span::plain(impl Into<String>) -> Span` and `Span::actor(impl Into<String>) -> Span`
  - `fn plain_text(impl Into<String>) -> Vec<Span>` (a single `Default` span)
  - Re-exported from crate root: `Role`, `Span`, `plain_text`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/snakewood-core/src/presentation.rs`:

```rust
#[test]
fn span_helpers_and_roles_round_trip() {
    assert_eq!(Span::plain("hi"), Span { text: "hi".to_string(), role: Role::Default });
    assert_eq!(Span::actor("a goblin"), Span { text: "a goblin".to_string(), role: Role::Actor });
    assert_eq!(plain_text("x"), vec![Span::plain("x")]);

    // serde round-trip for a styled span vec
    let spans = vec![Span::plain("You see "), Span::actor("a goblin")];
    let text = ron::ser::to_string(&spans).unwrap();
    let back: Vec<Span> = ron::from_str(&text).unwrap();
    assert_eq!(back, spans);
}
```

And a proptest for arbitrary span vecs (add to the same tests module; `proptest` is a dev-dependency of the crate):

```rust
use proptest::prelude::*;

fn arb_role() -> impl Strategy<Value = Role> {
    prop_oneof![Just(Role::Default), Just(Role::Actor)]
}

fn arb_span() -> impl Strategy<Value = Span> {
    (any::<String>(), arb_role()).prop_map(|(text, role)| Span { text, role })
}

proptest! {
    #[test]
    fn any_span_vec_round_trips(spans in prop::collection::vec(arb_span(), 0..8)) {
        let text = ron::ser::to_string(&spans).unwrap();
        let back: Vec<Span> = ron::from_str(&text).unwrap();
        prop_assert_eq!(back, spans);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p snakewood-core presentation::tests::span_helpers`
Expected: FAIL to compile ("cannot find type `Span`").

- [ ] **Step 3: Write minimal implementation**

At the top of `crates/snakewood-core/src/presentation.rs` (above `PresentationNode`), add:

```rust
/// Semantic role of a span of text. A growing vocabulary; the core emits only
/// `Default`/`Actor` in M2. Transports map roles to medium-specific styling.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Default,
    Actor,
}

/// A run of text carrying one semantic role.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub role: Role,
}

impl Span {
    pub fn plain(text: impl Into<String>) -> Span {
        Span { text: text.into(), role: Role::Default }
    }
    pub fn actor(text: impl Into<String>) -> Span {
        Span { text: text.into(), role: Role::Actor }
    }
}

/// A single `Default`-role span — the common case for plain/authored text.
pub fn plain_text(text: impl Into<String>) -> Vec<Span> {
    vec![Span::plain(text)]
}
```

- [ ] **Step 4: Re-export from the crate root**

In `crates/snakewood-core/src/lib.rs`, change the presentation re-export line `pub use presentation::PresentationNode;` to:

```rust
pub use presentation::{plain_text, PresentationNode, Role, Span};
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p snakewood-core presentation::tests`
Expected: PASS. Full `cargo test` still green (nothing else changed yet).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add crates/snakewood-core/src/presentation.rs crates/snakewood-core/src/lib.rs
git commit -m "feat(core): add Role, Span, and plain_text helper (presentation spans)"
```

---

## Task 2: Reshape free-text nodes to `Vec<Span>` (core + daemon, behavior-identical)

This is the atomic reshape: changing the four variant payloads breaks every construction/match site in the workspace, so all sites are fixed in one commit. **Telnet output stays byte-identical** — no ANSI yet (that's Task 3). The net observable behavior is unchanged; only the in-memory/serde shape of four node payloads changes.

**Files:**
- Modify: `crates/snakewood-core/src/presentation.rs` (enum + its serde test)
- Modify: `crates/snakewood-core/src/fabric/dispatch.rs` (production + tests)
- Modify: `crates/snakewood-core/src/fabric/operator.rs` (tests only)
- Modify: `crates/snakewood-core/tests/goblin_scenario.rs` (tests)
- Modify: `crates/snakewood-daemon/src/telnet/render.rs` (consume spans; keep output identical; tests)
- Modify: `crates/snakewood-daemon/src/engine.rs` (drain Denied construction + tests)
- Modify: `crates/snakewood-daemon/src/api/protocol.rs` (test fixture)
- Modify: `crates/snakewood-daemon/tests/engine_goblin.rs` (tests)

**Interfaces:**
- Consumes: `Span`, `Role`, `plain_text` (Task 1).
- Produces the reshaped enum:
  ```rust
  enum PresentationNode {
      RoomName(String),            // unchanged
      RoomDescription(Vec<Span>),  // was String
      Exits(Vec<Direction>),       // unchanged
      Occupants(Vec<Span>),        // was Vec<String>; each entry one Actor span
      Line(Vec<Span>),             // was String
      Denied(Vec<Span>),           // was String
      Prompt,                      // unchanged
  }
  ```

- [ ] **Step 1: Reshape the enum**

In `crates/snakewood-core/src/presentation.rs`, change the four variant payloads:

```rust
pub enum PresentationNode {
    RoomName(String),
    RoomDescription(Vec<Span>),
    Exits(Vec<Direction>),
    Occupants(Vec<Span>),
    Line(Vec<Span>),
    Denied(Vec<Span>),
    Prompt,
}
```

In the existing `presentation_node_round_trips_via_serde` test, change the `Line` case:

```rust
let line = PresentationNode::Line(plain_text("hello"));
```

(The `Exits` case in that test is unchanged.) Add `use super::*;` already imports `plain_text` via the module; if not, reference `crate::plain_text`.

- [ ] **Step 2: Update core production in `dispatch.rs`**

In `crates/snakewood-core/src/fabric/dispatch.rs`:

- `apply_effects` (the `Effect::Narrate` arm, ~line 27): change
  ```rust
  out.push((to, PresentationNode::Line(text.clone())));
  ```
  to
  ```rust
  out.push((to, PresentationNode::Line(crate::plain_text(text.clone()))));
  ```
- `room_presentation` (~lines 43, 54): change the description and occupants pushes:
  ```rust
  nodes.push(PresentationNode::RoomDescription(crate::plain_text(room.description.clone())));
  ```
  and for occupants, build `Actor` spans:
  ```rust
  let occupants: Vec<crate::Span> = {
      let mut names: Vec<String> = realm
          .mobs_in_room(room_id)
          .iter()
          .filter(|m| &m.id != viewer)
          .map(|m| m.name.clone())
          .collect();
      names.sort();
      names.into_iter().map(crate::Span::actor).collect()
  };
  nodes.push(PresentationNode::Occupants(occupants));
  ```
  (The `RoomName` and `Exits` pushes are unchanged.)
- The no-exit `Denied` push (~line 126): change
  ```rust
  PresentationNode::Denied(realm.no_exit_message.clone()),
  ```
  to
  ```rust
  PresentationNode::Denied(crate::plain_text(realm.no_exit_message.clone())),
  ```

- [ ] **Step 3: Update `dispatch.rs` test assertions**

In the `dispatch.rs` tests, wrap the changed-variant expected values (`RoomName` assertions are unchanged):
- The no-exit test (~line 225): `PresentationNode::Denied(crate::plain_text("You see no exit in that direction."))`
- The data-driven no-exit test (~line 241): `PresentationNode::Denied(crate::plain_text("There's nothing that way, friend."))`
- (`RoomName("The Old Well")` at ~207 and `RoomName("Snakewood Clearing")` at ~255 are unchanged.)

- [ ] **Step 4: Update `operator.rs` test constructors**

In `crates/snakewood-core/src/fabric/operator.rs` tests, the `Line` constructors (~lines 413, 415, 422, 424) change from `PresentationNode::Line("hi".into())` / `"bye".into()` to span form:
- `PresentationNode::Line(crate::plain_text("hi"))`
- `PresentationNode::Line(crate::plain_text("bye"))`
(matching both the input vec and the expected-output vec in `coalesce_leaves_unconfigured_kinds_untouched_and_ordered`). The `RoomName`/`Exits` constructors in those tests are unchanged. `PresentationKind::of` and all operator production code are unchanged.

- [ ] **Step 5: Update `goblin_scenario.rs` assertions**

In `crates/snakewood-core/tests/goblin_scenario.rs` (~lines 98, 134), change `PresentationNode::Line("The goblin blocks your way north.".to_string())` to `PresentationNode::Line(snakewood_core::plain_text("The goblin blocks your way north."))`.

- [ ] **Step 6: Update the telnet renderer to consume spans (output identical)**

In `crates/snakewood-daemon/src/telnet/render.rs`, add a span-flattening helper and update the changed arms so the produced strings are IDENTICAL to before:

```rust
use snakewood_core::{Direction, PresentationNode, Span};

fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}
```

Update `render_node`'s arms:
```rust
PresentationNode::RoomName(s) => Some(s.clone()),
PresentationNode::RoomDescription(spans) => Some(spans_text(spans)),
PresentationNode::Exits(dirs) => { /* unchanged */ }
PresentationNode::Occupants(spans) => {
    if spans.is_empty() {
        None
    } else {
        let names: Vec<&str> = spans.iter().map(|s| s.text.as_str()).collect();
        Some(format!("Also here: {}", names.join(", ")))
    }
}
PresentationNode::Line(spans) => Some(spans_text(spans)),
PresentationNode::Denied(spans) => Some(spans_text(spans)),
PresentationNode::Prompt => Some(">".to_string()),
```

Update the render tests' node constructors to the new shapes (the asserted output STRINGS are unchanged):
- `renders_room_view`: `RoomDescription(snakewood_core::plain_text("A clearing."))`, `Occupants(vec![Span::actor("a goblin")])` (import `Span`). `RoomName`/`Exits` unchanged. Assertion string unchanged.
- `empty_occupants_line_is_omitted`: `Occupants(vec![])` — `Vec<Span>` empty; assertion (`""`) unchanged.
- `renders_denied_and_line`: `Denied(snakewood_core::plain_text("You see no exit in that direction."))`, `Line(snakewood_core::plain_text("The goblin blocks your way north."))`. Assertion string unchanged.

- [ ] **Step 7: Update the daemon engine drain + tests**

In `crates/snakewood-daemon/src/engine.rs`:
- The drain rate-limit deny (~line 158): change `PresentationNode::Denied(text)` to `PresentationNode::Denied(snakewood_core::plain_text(text))`.
- Test at ~429: `PresentationNode::Denied(snakewood_core::plain_text("Too fast."))`.
- Test at ~550: `PresentationNode::Denied(snakewood_core::plain_text("You see no exit in that direction."))`.
- (`RoomName` assertions at ~465/470/492/533 are unchanged.)

- [ ] **Step 8: Update remaining daemon test fixtures**

- `crates/snakewood-daemon/src/api/protocol.rs` (~line 95): `messages: vec![PresentationNode::Line(snakewood_core::plain_text("hi"))]`.
- `crates/snakewood-daemon/tests/engine_goblin.rs` (~lines 97, 125): the two `PresentationNode::Line("...".to_string())` become `PresentationNode::Line(snakewood_core::plain_text("..."))` (keep the exact strings). The `RoomName` assertion at ~124 is unchanged.
- MCP (`mcp/dispatch.rs`, `mcp/tools.rs`) and `api/handler.rs`/`tests/api_e2e.rs` use only `RoomName`/`Exits` — no changes needed.

- [ ] **Step 9: Run the full suite to verify identical behavior**

Run: `cargo test`
Expected: PASS (same test count as before this task). If a render/e2e assertion fails, it means output changed — fix the construction, do NOT change asserted output strings (Plain output must stay byte-identical).

- [ ] **Step 10: Commit**

```bash
cargo fmt
git add crates/snakewood-core crates/snakewood-daemon
git commit -m "refactor(presentation): free-text nodes carry Vec<Span> (occupants=Actor); behavior identical"
```

---

## Task 3: Telnet ANSI rendering + `SNAKEWOOD_ANSI`

**Files:**
- Modify: `crates/snakewood-daemon/src/telnet/render.rs` (RenderStyle + ANSI + signature)
- Modify: `crates/snakewood-daemon/src/telnet/server.rs` (pass style)
- Modify: `crates/snakewood-daemon/src/mcp/tools.rs` (pass `Plain`)
- Modify: `crates/snakewood-daemon/src/main.rs` (`SNAKEWOOD_ANSI` env → style, thread to `serve`)
- Modify: `crates/snakewood-daemon/src/telnet/server.rs` + `mod.rs` as needed for the style parameter

**Interfaces:**
- Consumes: reshaped nodes + `Role`/`Span` (Task 2).
- Produces:
  - `enum RenderStyle { Ansi, Plain }` (in `telnet::render`, re-exported from `telnet`)
  - `fn render(nodes: &[PresentationNode], style: RenderStyle) -> String` (signature change from single-arg)
  - `serve(listener, engine, start_room, style: RenderStyle)` — style threaded to `handle_connection`.

- [ ] **Step 1: Write the failing tests**

In `crates/snakewood-daemon/src/telnet/render.rs` tests, add ANSI tests and convert the existing tests to pass `RenderStyle::Plain`:

```rust
#[test]
fn plain_mode_is_unstyled() {
    let nodes = vec![
        PresentationNode::RoomName("Snakewood Clearing".to_string()),
        PresentationNode::Occupants(vec![Span::actor("a goblin")]),
    ];
    let text = render(&nodes, RenderStyle::Plain);
    assert_eq!(text, "Snakewood Clearing\r\nAlso here: a goblin\r\n");
    assert!(!text.contains('\x1b'), "plain mode must emit no escape codes");
}

#[test]
fn ansi_mode_styles_roomname_bold_and_actor_cyan() {
    let nodes = vec![
        PresentationNode::RoomName("Snakewood Clearing".to_string()),
        PresentationNode::Occupants(vec![Span::actor("a goblin")]),
        PresentationNode::Denied(snakewood_core::plain_text("Nope.")),
    ];
    let text = render(&nodes, RenderStyle::Ansi);
    // RoomName is bold; the literal text survives between codes.
    assert!(text.contains("\x1b[1mSnakewood Clearing\x1b[0m"));
    // Actor span is cyan.
    assert!(text.contains("\x1b[36ma goblin\x1b[0m"));
    // Denied line is red.
    assert!(text.contains("\x1b[31mNope.\x1b[0m"));
    // Substring of the raw words still present (so e2e substring matches survive).
    assert!(text.contains("Snakewood Clearing"));
}
```

Update the three existing render tests (`renders_room_view`, `empty_occupants_line_is_omitted`, `no_exits_says_none`, `renders_denied_and_line`) to call `render(&nodes, RenderStyle::Plain)` — their asserted strings are unchanged.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p snakewood-daemon --lib telnet::render`
Expected: FAIL to compile (`render` takes one arg; `RenderStyle` undefined).

- [ ] **Step 3: Implement `RenderStyle` + ANSI**

Rewrite `crates/snakewood-daemon/src/telnet/render.rs`'s rendering to take a style. Add the enum and SGR helpers:

```rust
use snakewood_core::{Direction, PresentationNode, Role, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderStyle {
    Ansi,
    Plain,
}

const RESET: &str = "\x1b[0m";

fn role_sgr(role: Role) -> Option<&'static str> {
    match role {
        Role::Default => None,
        Role::Actor => Some("\x1b[36m"), // cyan
    }
}

/// Wrap `text` in `sgr` + reset when Ansi and a code is given; else return text.
fn styled(text: &str, sgr: Option<&str>, style: RenderStyle) -> String {
    match (style, sgr) {
        (RenderStyle::Ansi, Some(code)) => format!("{code}{text}{RESET}"),
        _ => text.to_string(),
    }
}

/// Render one span, applying its role colour in Ansi mode.
fn render_span(span: &Span, style: RenderStyle) -> String {
    styled(&span.text, role_sgr(span.role), style)
}
```

Then update `render_node` to take `style` and apply per-variant base styling (bold title, red denial, dim exits/prompt) and per-span role styling for span nodes:

```rust
fn render_node(node: &PresentationNode, style: RenderStyle) -> Option<String> {
    match node {
        PresentationNode::RoomName(s) => Some(styled(s, Some("\x1b[1m"), style)), // bold
        PresentationNode::RoomDescription(spans) => {
            Some(spans.iter().map(|sp| render_span(sp, style)).collect())
        }
        PresentationNode::Exits(dirs) => {
            let body = if dirs.is_empty() {
                "Exits: none".to_string()
            } else {
                let names: Vec<&str> = dirs.iter().map(direction_name).collect();
                format!("Exits: {}", names.join(", "))
            };
            Some(styled(&body, Some("\x1b[2m"), style)) // dim
        }
        PresentationNode::Occupants(spans) => {
            if spans.is_empty() {
                None
            } else {
                let names: Vec<String> = spans.iter().map(|sp| render_span(sp, style)).collect();
                Some(format!("Also here: {}", names.join(", ")))
            }
        }
        PresentationNode::Line(spans) => {
            Some(spans.iter().map(|sp| render_span(sp, style)).collect())
        }
        PresentationNode::Denied(spans) => {
            // Denial base style is red; spans are Default so no nested colour.
            let body: String = spans.iter().map(|sp| sp.text.as_str()).collect();
            Some(styled(&body, Some("\x1b[31m"), style)) // red
        }
        PresentationNode::Prompt => Some(styled(">", Some("\x1b[2m"), style)), // dim
    }
}

/// Render a batch of presentation nodes to telnet wire text (CRLF line endings).
pub fn render(nodes: &[PresentationNode], style: RenderStyle) -> String {
    let mut out = String::new();
    for node in nodes {
        if let Some(line) = render_node(node, style) {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    out
}
```

Note: in `Plain` mode `styled` returns the bare text and `role_sgr` output is ignored, so output is byte-identical to M1. Remove the now-unused `spans_text` helper from Task 2 if the compiler flags it (its uses are replaced above).

- [ ] **Step 4: Re-export `RenderStyle`**

In `crates/snakewood-daemon/src/telnet/mod.rs`, change `pub use render::render;` to `pub use render::{render, RenderStyle};`.

- [ ] **Step 5: Thread the style through the telnet server**

In `crates/snakewood-daemon/src/telnet/server.rs`:
- Change `serve`'s signature to `pub async fn serve(listener: TcpListener, engine: Rc<RefCell<Engine>>, start_room: EntityId, style: RenderStyle)` and pass `style` into each `handle_connection(stream, engine, start_room, style)` call (clone/copy `style` — it's `Copy`).
- Change `handle_connection`'s signature to accept `style: RenderStyle`, and change the two `render(&e.poll(sid))` / `render(&...)` calls (greeting is enqueued so only the flush branch renders) to `render(&e.poll(sid), style)`. Import `RenderStyle` via `use crate::telnet::{..., RenderStyle}` or `use super::RenderStyle`.

- [ ] **Step 6: Update the MCP text renderer to `Plain`**

In `crates/snakewood-daemon/src/mcp/tools.rs`, `response_to_text` calls `render(view)` / `render(messages)`. Change both to `render(view, RenderStyle::Plain)` / `render(messages, RenderStyle::Plain)` and update the import to `use crate::telnet::{render, RenderStyle};`. (MCP tool text stays plain — no escape codes in structured tool output.)

- [ ] **Step 7: Wire `SNAKEWOOD_ANSI` in `main.rs`**

In `crates/snakewood-daemon/src/main.rs`:
- Add `use snakewood_daemon::telnet::{run_tick_loop, serve, RenderStyle};` (extend the existing telnet import).
- After the other env reads, add:
  ```rust
  let render_style = match std::env::var("SNAKEWOOD_ANSI").ok().as_deref() {
      Some("0") | Some("false") | Some("off") => RenderStyle::Plain,
      _ => RenderStyle::Ansi, // default on
  };
  ```
- Pass `render_style` into the `serve(...)` call: `serve(listener, engine.clone(), start_room.clone(), render_style)`.

- [ ] **Step 8: Fix the telnet e2e test's `serve` call**

In `crates/snakewood-daemon/tests/telnet_e2e.rs`, the `spawn_local(serve(...))` call now needs a style argument. Use `RenderStyle::Plain` so the substring assertions ("Snakewood Clearing", "The Old Well", "What?") match without escape codes:
- Add `use snakewood_daemon::telnet::RenderStyle;`
- Change the serve spawn to `serve(listener, engine.clone(), id("snakewood/clearing"), RenderStyle::Plain)`.

- [ ] **Step 9: Run the full suite**

Run: `cargo test`
Expected: PASS (all crates). Then run the ANSI render tests specifically: `cargo test -p snakewood-daemon --lib telnet::render` — PASS.

- [ ] **Step 10: Manual ANSI smoke (optional but recommended)**

Boot with ANSI and eyeball colour, then with it off:
```bash
SNAKEWOOD_DATA=/private/tmp/claude-502/-Users-nathan-Projects-snakewood/fb1a5eb9-51b1-4f64-959d-915befc6011a/scratchpad/m2pres \
SNAKEWOOD_ADDR=127.0.0.1:4700 SNAKEWOOD_API_ADDR=127.0.0.1:4701 timeout 2 cargo run -p snakewood-daemon &
sleep 1; printf 'look\nquit\n' | nc 127.0.0.1 4700 | cat -v | head
```
Expected (Ansi default): room name wrapped in `^[[1m…^[[0m`, occupant names in `^[[36m…^[[0m`. Re-run with `SNAKEWOOD_ANSI=0` prepended → no `^[[` codes.

- [ ] **Step 11: Commit**

```bash
cargo fmt
git add crates/snakewood-daemon
git commit -m "feat(daemon): telnet ANSI rendering via role/variant style tables (SNAKEWOOD_ANSI)"
```

---

## Self-Review Notes (for the implementer)

- **Never rename/reorder `PresentationNode` variants** — operators match by variant. If you find yourself editing `operator.rs` production code (not tests), stop: only the `Line` test constructors in `operator.rs` should change.
- **`Plain` output is a contract** — if a Plain-mode assertion's string needs changing to pass, something is wrong in `render_node`; fix the renderer, not the assertion.
- **Task 2 must keep the same test count and identical outputs** — it's a shape refactor, not a behavior change. Task 3 is where visible (ANSI) behavior changes, guarded by new tests.
- **Deferred (out of scope, backlog):** inline roles for authored text (RON descriptions, `Narrate` effects) via a role-markup syntax or structured authored spans; additional roles (Exit, Item, Emphasis, Danger, Speech); a structured (non-flattened) MCP presentation channel. Do not build these.
