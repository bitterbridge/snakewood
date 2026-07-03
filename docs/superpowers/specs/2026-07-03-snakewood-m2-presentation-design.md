# Snakewood M2 — Richer Presentation Vocabulary — Design

**Status:** Approved (design). Ready for implementation planning.
**Date:** 2026-07-03
**Parent milestone:** M2 (spec §8). Second of three M2 sub-projects (Operators [done] · **richer presentation vocabulary** · SSH/WS gateways), each its own spec → plan → implementation cycle.
**One-line:** Introduce the design's deferred **semantic styled-span model** — free-text presentation carries `Span`s tagged with a semantic `Role` — so one core output renders to telnet ANSI, is consumable as structure by MCP/API, and is ready for a WS gateway to render as HTML/CSS.

---

## 1. Goal & Motivation

The design spec (`2026-07-02-snakewood-mud-design.md`) §2.1 describes the presentation layer as a **typed presentation tree with styled spans carrying semantic roles** — the core emits *meaning* ("this span is *danger*", "this is *player speech*"), and each transport renders it to its medium. M1 shipped a deliberately **minimal** node set with flat `String` payloads (`RoomName`/`RoomDescription`/`Exits`/`Occupants`/`Line`/`Denied`/`Prompt`) and explicitly deferred styled spans.

This sub-project makes that structural leap. It is the natural predecessor to the **SSH/WS gateways** (the third M2 sub-project): a WS gateway renders HTML/CSS meaningfully only when the presentation stream carries semantic roles it can map to CSS classes. Telnet gains ANSI colour from the same roles; MCP continues to consume structure.

**Goal:** establish the span/role seam end-to-end — a `Role` vocabulary, a `Span` type, `Vec<Span>` payloads on the free-text nodes, a per-transport role→style mapping, and telnet ANSI rendering — proven against the content that exists today (room views, narration, denials, occupants).

**Non-goals (deferred):**
- Enriching **authored** text (RON room descriptions, `Narrate` effect text) with inline role markup — needs the effect/authoring vocabulary to carry roles; a later step. In M2 authored text is a single `Default` span.
- Roles beyond `Default`/`Actor` (Exit, Item, Emphasis, Danger, Speech, System…) — added agile-style when content produces them (M3+).
- The SSH/WS gateways themselves (next sub-project).
- Any change to the event fabric, operators, or the engine.

## 2. Chosen shape — hybrid model

Considered three integrations (recorded for context):
- **Hybrid (CHOSEN):** keep the coarse-role *variants* as the message "kind" (each maps to a base style); convert only the free-text payloads that can hold mixed roles into `Vec<Span>`.
- **Uniform `Message { kind, spans }`:** collapse variants into one type. Conceptually clean but a large rewrite of every consumer and the operator `PresentationKind` mapping, for little near-term gain.
- **Additive `Styled(Vec<Span>)` variant:** least churn now, but leaves existing content unstyled and creates two parallel text representations (tech debt).

Hybrid is chosen: the current variants *already* encode coarse roles (`RoomName` = title, `Denied` = error, `Exits` = exit list), so a transport can style by variant. The genuine gap is **inline roles inside free text**, which is exactly what the span payloads add — with minimal churn and no disruption to operators.

## 3. The model (`snakewood-core::presentation`)

```rust
/// Semantic role of a span. A growing vocabulary; only Default/Actor are
/// emitted by the core in M2. Transports map roles to medium-specific styling.
enum Role {
    Default,
    Actor,
}

/// A run of text with one semantic role.
struct Span {
    text: String,
    role: Role,
}

impl Span {
    fn plain(text: impl Into<String>) -> Span;  // role: Default
    fn actor(text: impl Into<String>) -> Span;   // role: Actor
}

/// Convenience: a single Default span. Keeps call sites building plain text terse.
fn plain(text: impl Into<String>) -> Vec<Span>;   // vec![Span::plain(text)]

enum PresentationNode {
    RoomName(String),            // atomic title — styled by variant, no span
    RoomDescription(Vec<Span>),  // was String
    Exits(Vec<Direction>),       // structured — styled by variant
    Occupants(Vec<Span>),        // was Vec<String>; each entry one Actor-role span
    Line(Vec<Span>),             // was String — narration
    Denied(Vec<Span>),           // was String
    Prompt,
}
```

`Span` and `Role` derive `Serialize, Deserialize, Debug, Clone, PartialEq`, matching the existing node derives. RON/JSON round-trip is preserved.

**Operators are untouched.** `PresentationKind::of` matches by *variant*, and the variant set/names/order are unchanged — only payload *types* change. `coalesce` (keyed by `PresentationKind`) and `RateLimit` keep working with no edits. This is a deliberate constraint on the design: do not rename or reorder `PresentationNode` variants.

## 4. Where roles come from (core `dispatch`)

The core auto-assigns roles only to content whose meaning it structurally knows:

- **`Occupants`**: each co-present mob's name → `Span::actor(name)`. This is the one place the `Actor` role is actively emitted in M2.
- **`RoomDescription`**, **`Line`** (narration from `Effect::Narrate`), **`Denied`** (no-exit fallback, rate-limit deny): built from an authored/config `String` → `plain(string)` (single `Default` span).
- **`RoomName`**, **`Exits`**, **`Prompt`**: unchanged payloads; styled by variant at the transport.

`dispatch.rs` `room_presentation` and the narration/deny construction sites change to build spans via the helpers. The daemon `Engine::drain`'s rate-limit `Denied(text)` construction likewise becomes `Denied(plain(text))`.

## 5. Telnet rendering (`snakewood-daemon::telnet::render`)

`render` gains a style mode:

```rust
enum RenderStyle { Ansi, Plain }
fn render(nodes: &[PresentationNode], style: RenderStyle) -> String;
```

- **`Plain`**: byte-for-byte identical to today's output (spans are concatenated by `.text`; the existing per-variant line formatting — `"Exits: …"`, `"Also here: …"`, omit-empty-occupants, `"Exits: none"` — is preserved). This is the regression guard and the `nc`/smoke-test mode.
- **`Ansi`** (production default): wraps text in SGR escape codes from two tables — a **role→SGR** map (`Actor` → a distinct colour, e.g. cyan) applied per span, and a **variant→base style** (`RoomName` → bold; `Denied` → red; `Exits`/`Prompt` → dim) applied to the line/segment. Each styled run is reset (`\x1b[0m`) so codes never bleed across lines. The literal text is emitted intact between codes, so substring matching still works.
- Selected by `SNAKEWOOD_ANSI` env in `main.rs` (default on → `Ansi`; set to `0`/`false` → `Plain`). The per-connection handler passes the chosen `RenderStyle` to `render`.

Rationale for ANSI-by-default: MUD clients universally support ANSI; the `Plain` toggle serves raw `nc`/telnet and tests.

## 6. Structured consumers (MCP / JSON API)

- **JSON API** (`api/protocol`, `api/handler`): `PresentationNode` is serialised as-is; the wire shape changes (`RoomDescription`/`Occupants`/`Line`/`Denied` now carry span arrays). API integration tests update their expected node shapes. A future WS gateway consumes this same structured stream.
- **MCP** (`mcp/tools`): tool-result *content* is text, so `Vec<Span>` is **flattened** to concatenated `.text` (roles ignored for the text view; still present in the structure for a future structured MCP client). MCP behaviour (dig/move/look tool text) is unchanged from the player's perspective.

## 7. Testing Strategy (cheapest layer first)

- **Core unit (the bulk):**
  - `Span`/`Role`/reshaped `PresentationNode` serde round-trip (proptest over generated spans + a golden RON snapshot check).
  - `dispatch` emits `Occupants` as `Actor`-role spans and `RoomDescription`/`Line`/`Denied` as single `Default` spans; the blocking-goblin scenario stays green with the new shapes.
- **Telnet render:**
  - `Plain` mode reproduces the exact M1 strings (the existing render tests, updated to pass `RenderStyle::Plain`, must still assert the identical bytes).
  - `Ansi` mode wraps `Actor` spans and styled variants in the expected SGR codes and resets; the literal words remain present as substrings.
- **MCP:** span-flattening yields the same tool-result text as before the change.
- **End-to-end (few):** telnet (`Plain` and/or substring-tolerant `Ansi`), JSON API, and MCP bridge stay green.

## 8. Implementation Staging (indicative — firmed up in the plan)

1. **Core model + dispatch reshape:** `Role`, `Span` (+ helpers), reshaped `PresentationNode`; update `dispatch` (occupants → Actor spans; other free text → Default); round-trip + dispatch + goblin tests. Fix all core call sites so the crate compiles.
2. **Telnet ANSI renderer:** `RenderStyle`, role→SGR + variant→base-style tables, `Plain`-equals-M1 regression tests + `Ansi` tests; `SNAKEWOOD_ANSI` wired in `main.rs` and passed through the connection handler.
3. **Structured consumers + e2e:** MCP span-flattening; JSON API test shape updates; daemon `Engine::drain` deny construction; full telnet/api/mcp e2e green.

## 9. Success Criteria (all testable)

1. `Span`/`Role`/reshaped nodes round-trip losslessly (proptest + golden green); no `PresentationNode` variant was renamed or reordered (operators still compile and pass unchanged).
2. `dispatch` produces `Occupants` entries as `Actor`-role spans; `RoomDescription`/`Line`/`Denied` as `Default` spans.
3. Telnet `Plain` render is byte-identical to M1 output; `Ansi` render wraps roles/variants in the expected SGR codes with resets, text intact.
4. MCP tool-result text is unchanged (spans flattened); JSON API exposes the structured span shape.
5. Whole suite green; no changes to the event fabric, operators, or engine tick/drain logic.
