# Snakewood M1 Stage 3d-mcp — MCP Bridge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `snakewood-mcp`, a thin synchronous binary that speaks MCP (JSON-RPC 2.0 over stdio) to Claude Code and forwards `look`/`move`/`dig` tool calls to the daemon's JSON command API over a persistent TCP connection — attaching to a stable, persistent builder character so restarting the bridge never disturbs the running world. Completing this completes M1.

**Architecture:** The daemon API gains `ConnectAs { actor }` — attach a session to a named, persistent character (created at the start room if absent), whose mob is NOT despawned when the connection drops (only the session is). The bridge is a separate, synchronous binary (`src/bin/snakewood-mcp.rs`) reusing `snakewood_daemon::api::{ApiRequest, ApiResponse}`: it holds one `std::net::TcpStream` to the daemon (reconnecting on failure, re-`ConnectAs`-ing the builder), reads JSON-RPC lines from stdin, dispatches `initialize`/`tools/list`/`tools/call` (pure `dispatch_rpc` over a `DaemonClient` trait, unit-tested with a mock), and writes JSON-RPC results to stdout. Hand-rolled minimal MCP — no SDK.

**Tech Stack:** Rust (edition 2021), `serde` + `serde_json` (already deps), `std::net`/`std::io` (sync — NO tokio in the bridge). The daemon binary/API are unchanged except the additive `ConnectAs`.

## Global Constraints

- **Language:** Rust, edition 2021. Work in `snakewood-daemon` (additive `ConnectAs` on the API; a new `mcp` lib module; a second binary `src/bin/snakewood-mcp.rs`). No new dependencies (`serde`, `serde_json`, `tokio` already present; the bridge uses only `std` + serde).
- **The bridge is synchronous:** `std::net::TcpStream` + `std::io` stdin/stdout. No tokio in the bridge binary or the `mcp` module. (The daemon stays async; the bridge is a plain sync client.)
- **MCP protocol:** JSON-RPC 2.0, newline-delimited, over stdio. Implement `initialize` (→ protocolVersion `"2024-11-05"`, `capabilities.tools`, `serverInfo`), `notifications/initialized` (notification → no response), `tools/list`, `tools/call`. Tools: `snakewood_look`, `snakewood_move` (arg `direction`), `snakewood_dig` (args `direction`, `id`, `name`, `description`). Tool results are `{ "content": [ { "type": "text", "text": ... } ], "isError": bool }`.
- **Stable builder identity:** the bridge attaches via `ConnectAs { actor: "player/mcp-builder" }`. On a fresh daemon the builder is created at the start room + checkpointed; on reconnect it re-attaches to the persisted character. Dropping the bridge connection must NOT remove the builder mob (only its session).
- **Reuse the API contract:** the bridge speaks the exact `ApiRequest`/`ApiResponse` JSON the daemon already serves; no new daemon transport.
- **Testability:** the JSON-RPC dispatch is a pure function over a `DaemonClient` trait, unit-tested with a mock client (no real stdio/TCP). An integration test drives it against a real in-process daemon API.
- **`World`/core behavior unchanged**; `BTreeMap`/`BTreeSet` only. **No `RefCell` borrow across an `.await`** in the (async) daemon API changes.
- **Deferred (do NOT build):** MCP resources/prompts (tools only), auth, multiple concurrent builders, MCP over anything but stdio, streaming/progress. Real Claude-Code-connects verification is a manual post-merge step (add to MCP config).

---

### Task 1: daemon API — `ConnectAs { actor }` + persistent (session-only) disconnect

**Files:**
- Modify: `crates/snakewood-daemon/src/api/protocol.rs`
- Modify: `crates/snakewood-daemon/src/api/handler.rs`
- Modify: `crates/snakewood-daemon/src/api/server.rs`
- Modify: `crates/snakewood-daemon/src/telnet/provision.rs` (add a named-spawn helper)

**Interfaces:**
- Consumes: `Engine`, `SessionId`, `spawn_player`; `snakewood_core::{EntityId, Mob, Flag, Intent}`.
- Produces:
  - `ApiRequest::ConnectAs { actor: String }` (new variant).
  - `telnet::provision::attach_named(engine: &mut Engine, actor_id: &EntityId, start_room: &EntityId) -> SessionId` — if `actor_id`'s mob exists, just `connect` a session to it; else insert a living/conscious mob named after the id at `start_room`, then `connect`. Returns the session.
  - `handle_api_request` handles `ConnectAs { actor }` → validate the id → `attach_named` → submit `Look` → `Connected { session, actor, view }` (or `Error` on invalid id).
  - `serve_api`'s cleanup despawns (mob+session) only sessions created by `Connect`; sessions created by `ConnectAs` are cleaned up with `engine.disconnect(session)` only (the named mob PERSISTS).

- [ ] **Step 1: Add the `ConnectAs` variant to `protocol.rs`**

In `crates/snakewood-daemon/src/api/protocol.rs`, add to `ApiRequest` (after `Connect`):

```rust
    ConnectAs { actor: String },
```

Add a protocol round-trip test:

```rust
    #[test]
    fn connect_as_round_trips() {
        let req = ApiRequest::ConnectAs { actor: "player/mcp-builder".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"op\":\"connect_as\""));
        let back: ApiRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }
```

- [ ] **Step 2: Add `attach_named` to `provision.rs` with tests**

In `crates/snakewood-daemon/src/telnet/provision.rs`, add:

```rust
/// Attach a session to a persistent named actor, creating its mob at
/// `start_room` if it doesn't exist yet. Unlike `spawn_player`, the mob is NOT
/// removed when the session ends (see the API server's cleanup).
pub fn attach_named(engine: &mut Engine, actor_id: &EntityId, start_room: &EntityId) -> SessionId {
    if engine.realm().mob(actor_id).is_none() {
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        flags.insert(Flag::Conscious);
        engine.realm_mut().insert_mob(Mob {
            id: actor_id.clone(),
            name: actor_id.name().to_string(),
            location: start_room.clone(),
            flags,
            responders: Vec::new(),
        });
    }
    engine.connect(actor_id.clone())
}
```

Add tests:

```rust
    #[test]
    fn attach_named_creates_then_reuses() {
        let mut engine = Engine::new(Realm::new(World::default()), Box::new(ManualClock::new(0)));
        let start = EntityId::new("snakewood/clearing").unwrap();
        let builder = EntityId::new("player/mcp-builder").unwrap();
        let s1 = attach_named(&mut engine, &builder, &start);
        assert_eq!(engine.session_actor(s1), Some(&builder));
        assert_eq!(engine.realm().mob_location(&builder).map(|r| r.as_str()), Some("snakewood/clearing"));
        // A second attach reuses the SAME mob (not recreated) and binds a new session.
        let s2 = attach_named(&mut engine, &builder, &EntityId::new("snakewood/elsewhere").unwrap());
        assert_ne!(s1, s2);
        // location unchanged — the mob was reused, not moved/recreated.
        assert_eq!(engine.realm().mob_location(&builder).map(|r| r.as_str()), Some("snakewood/clearing"));
    }
```

(Ensure the test module imports `Realm`, `World`, `ManualClock`, `EntityId` as the existing provision tests do.)

- [ ] **Step 3: Handle `ConnectAs` in `handler.rs`**

Add a `use crate::telnet::attach_named;` (merge with the existing `use crate::telnet::{despawn_player, spawn_player};` → `{attach_named, despawn_player, spawn_player}`). Add a match arm to `handle_api_request` (after the `Connect` arm):

```rust
        ApiRequest::ConnectAs { actor } => {
            let actor_id = match EntityId::new(actor.clone()) {
                Ok(id) => id,
                Err(_) => return ApiResponse::Error { message: format!("invalid actor id: {actor}") },
            };
            let sid = attach_named(engine, &actor_id, start_room);
            engine.submit(sid, Intent::Look { actor: actor_id.clone() });
            let view = engine.poll(sid);
            ApiResponse::Connected { session: sid.0, actor: actor_id.to_string(), view }
        }
```

Add a handler test:

```rust
    #[test]
    fn connect_as_attaches_named_builder() {
        let mut e = engine();
        let resp = handle_api_request(&mut e, ApiRequest::ConnectAs { actor: "player/mcp-builder".to_string() }, &start());
        match resp {
            ApiResponse::Connected { actor, view, .. } => {
                assert_eq!(actor, "player/mcp-builder");
                assert!(view.contains(&PresentationNode::RoomName("Snakewood Clearing".to_string())));
            }
            other => panic!("expected Connected, got {other:?}"),
        }
    }
```

- [ ] **Step 4: Persistent cleanup in `server.rs`**

In `crates/snakewood-daemon/src/api/server.rs`, the connection currently tracks `created: Vec<SessionId>` (all despawned on exit). Split tracking so `ConnectAs` sessions are cleaned up session-only. Change the tracking to record whether each created session is ephemeral. Replace the `created` bookkeeping with two vecs and adjust the loop + cleanup:

- Add `let mut ephemeral: Vec<SessionId> = Vec::new();` and `let mut persistent: Vec<SessionId> = Vec::new();` (replacing the single `created`).
- When the response is `ApiResponse::Connected { session, .. }`, push to `ephemeral` if the request was `ApiRequest::Connect`, or to `persistent` if it was `ApiRequest::ConnectAs { .. }`.
- Keep the existing `Disconnect { session }` handling that removes a session from tracking (remove from BOTH vecs via `retain`).
- In the always-run cleanup: for each `sid` in `ephemeral`, look up its actor and `despawn_player` (as today); for each `sid` in `persistent`, call `engine.disconnect(sid)` only (do NOT remove the mob).

Concretely, the request-type check that currently produces the `Disconnect` id should be widened to also classify `Connect` vs `ConnectAs`. Implement by capturing the request kind before dispatch:

```rust
        // Classify the request so cleanup knows how to treat any session it creates.
        let kind = match &parsed {
            Ok(ApiRequest::Connect) => RequestKind::Ephemeral,
            Ok(ApiRequest::ConnectAs { .. }) => RequestKind::Persistent,
            Ok(ApiRequest::Disconnect { session }) => RequestKind::Disconnected(SessionId(*session)),
            _ => RequestKind::Other,
        };
```

where `parsed: Result<ApiRequest, _>` is the `serde_json::from_str` result (parse once, reuse for both classification and dispatch), and:

```rust
enum RequestKind {
    Ephemeral,
    Persistent,
    Disconnected(SessionId),
    Other,
}
```

After dispatch, on `ApiResponse::Connected { session, .. }`: `match kind { RequestKind::Ephemeral => ephemeral.push(SessionId(session)), RequestKind::Persistent => persistent.push(SessionId(session)), _ => {} }`. On `RequestKind::Disconnected(sid)`: `ephemeral.retain(|s| *s != sid); persistent.retain(|s| *s != sid);`.

Cleanup block (after the loop, always runs):
```rust
    {
        let mut e = engine.borrow_mut();
        for sid in ephemeral {
            if let Some(actor) = e.session_actor(sid).cloned() {
                despawn_player(&mut e, sid, &actor);
            }
        }
        for sid in persistent {
            e.disconnect(sid);
        }
    }
```

Keep the borrow discipline (no borrow across `.await`). Import `SessionId` if not already.

- [ ] **Step 5: Run tests + fmt + clippy**

Run: `cargo test -p snakewood-daemon` (all pass, incl. new `connect_as_*`/`attach_named_*`), then `cargo fmt -p snakewood-daemon && cargo fmt -p snakewood-daemon -- --check && cargo clippy -p snakewood-daemon --all-targets -- -D warnings`.

- [ ] **Step 6: Commit**

```bash
git add crates/snakewood-daemon/src/api/protocol.rs crates/snakewood-daemon/src/api/handler.rs crates/snakewood-daemon/src/api/server.rs crates/snakewood-daemon/src/telnet/provision.rs
git commit -m "feat: add ConnectAs for persistent named actors (stable MCP builder identity)"
```

---

### Task 2: MCP JSON-RPC types + tool mapping (pure)

**Files:**
- Create: `crates/snakewood-daemon/src/mcp/mod.rs`
- Create: `crates/snakewood-daemon/src/mcp/protocol.rs`
- Create: `crates/snakewood-daemon/src/mcp/tools.rs`
- Modify: `crates/snakewood-daemon/src/lib.rs`

**Interfaces:**
- Consumes: `serde_json::Value`; `snakewood_daemon::api::{ApiRequest, ApiResponse}`; `snakewood_core::{Direction, PresentationNode}`.
- Produces:
  - `mcp::protocol`: `JsonRpcRequest { jsonrpc: String, id: Option<Value>, method: String, params: Option<Value> }`, `JsonRpcResponse` (with `result` or `error`), `JsonRpcError { code: i64, message: String }`, all serde. A `JsonRpcResponse::ok(id, result)` and `::error(id, code, message)` constructor.
  - `mcp::tools`: `pub fn tool_definitions() -> serde_json::Value` (the `tools/list` array: `snakewood_look`, `snakewood_move`, `snakewood_dig` with input schemas); `pub fn tool_call_to_request(name: &str, args: &Value, session: u64) -> Result<ApiRequest, String>`; `pub fn response_to_text(resp: &ApiResponse) -> (String, bool)` — renders an `ApiResponse` to (human text, is_error) for the MCP tool result (reuse `crate::telnet::render` for the presentation nodes).

- [ ] **Step 1: Write `crates/snakewood-daemon/src/mcp/protocol.rs` with tests**

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON-RPC 2.0 request (or notification when `id` is absent).
#[derive(Deserialize, Debug, Clone)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 response.
#[derive(Serialize, Debug, Clone)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Serialize, Debug, Clone)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

impl JsonRpcResponse {
    pub fn ok(id: Value, result: Value) -> JsonRpcResponse {
        JsonRpcResponse { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    pub fn error(id: Value, code: i64, message: impl Into<String>) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message: message.into() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_request() {
        let req: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap();
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(Value::from(1)));
    }

    #[test]
    fn notification_has_no_id() {
        let req: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        assert!(req.id.is_none());
    }

    #[test]
    fn ok_response_serializes_without_error_field() {
        let resp = JsonRpcResponse::ok(Value::from(1), serde_json::json!({"x": 1}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"result\""));
        assert!(!json.contains("\"error\""));
    }
}
```

- [ ] **Step 2: Write `crates/snakewood-daemon/src/mcp/tools.rs` with tests**

```rust
use serde_json::{json, Value};

use snakewood_core::Direction;

use crate::api::{ApiRequest, ApiResponse};
use crate::telnet::render;

/// Parse a direction word (as the tool arg / API uses PascalCase on the wire,
/// but tool users type lowercase) into a `Direction`.
fn parse_direction(s: &str) -> Option<Direction> {
    match s.trim().to_ascii_lowercase().as_str() {
        "north" | "n" => Some(Direction::North),
        "south" | "s" => Some(Direction::South),
        "east" | "e" => Some(Direction::East),
        "west" | "w" => Some(Direction::West),
        "up" | "u" => Some(Direction::Up),
        "down" | "d" => Some(Direction::Down),
        _ => None,
    }
}

/// The MCP `tools/list` payload.
pub fn tool_definitions() -> Value {
    json!({
        "tools": [
            {
                "name": "snakewood_look",
                "description": "Look at the current room (name, description, exits, occupants).",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "snakewood_move",
                "description": "Move the builder in a direction (north/south/east/west/up/down).",
                "inputSchema": {
                    "type": "object",
                    "properties": { "direction": { "type": "string" } },
                    "required": ["direction"]
                }
            },
            {
                "name": "snakewood_dig",
                "description": "Dig a new room in a direction, linked both ways, and persist it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "direction": { "type": "string" },
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "description": { "type": "string" }
                    },
                    "required": ["direction", "id", "name", "description"]
                }
            }
        ]
    })
}

/// Map an MCP tool call to an `ApiRequest` for `session`.
pub fn tool_call_to_request(name: &str, args: &Value, session: u64) -> Result<ApiRequest, String> {
    let dir = |args: &Value| -> Result<Direction, String> {
        let s = args.get("direction").and_then(|v| v.as_str()).ok_or("missing 'direction'")?;
        parse_direction(s).ok_or_else(|| format!("bad direction: {s}"))
    };
    match name {
        "snakewood_look" => Ok(ApiRequest::Look { session }),
        "snakewood_move" => Ok(ApiRequest::Move { session, direction: dir(args)? }),
        "snakewood_dig" => {
            let get = |k: &str| args.get(k).and_then(|v| v.as_str()).map(str::to_string);
            Ok(ApiRequest::Dig {
                session,
                direction: dir(args)?,
                id: get("id").ok_or("missing 'id'")?,
                name: get("name").ok_or("missing 'name'")?,
                description: get("description").ok_or("missing 'description'")?,
            })
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// Render an `ApiResponse` to (text, is_error) for an MCP tool result.
pub fn response_to_text(resp: &ApiResponse) -> (String, bool) {
    match resp {
        ApiResponse::Connected { view, .. } => (render(view), false),
        ApiResponse::Ok { messages } => {
            let text = render(messages);
            (if text.is_empty() { "OK".to_string() } else { text }, false)
        }
        ApiResponse::Error { message } => (message.clone(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snakewood_core::PresentationNode;

    #[test]
    fn move_tool_maps_to_request() {
        let req = tool_call_to_request("snakewood_move", &json!({"direction": "north"}), 5).unwrap();
        assert_eq!(req, ApiRequest::Move { session: 5, direction: Direction::North });
    }

    #[test]
    fn dig_tool_maps_all_fields() {
        let args = json!({"direction":"east","id":"snakewood/hollow","name":"A Hollow","description":"Mossy."});
        let req = tool_call_to_request("snakewood_dig", &args, 1).unwrap();
        assert_eq!(req, ApiRequest::Dig {
            session: 1, direction: Direction::East,
            id: "snakewood/hollow".to_string(), name: "A Hollow".to_string(), description: "Mossy.".to_string(),
        });
    }

    #[test]
    fn unknown_tool_and_bad_direction_error() {
        assert!(tool_call_to_request("nope", &json!({}), 0).is_err());
        assert!(tool_call_to_request("snakewood_move", &json!({"direction":"sideways"}), 0).is_err());
    }

    #[test]
    fn response_text_marks_errors() {
        let (text, is_err) = response_to_text(&ApiResponse::Error { message: "boom".to_string() });
        assert_eq!(text, "boom");
        assert!(is_err);
        let (text, is_err) = response_to_text(&ApiResponse::Ok {
            messages: vec![PresentationNode::RoomName("The Old Well".to_string())],
        });
        assert!(text.contains("The Old Well"));
        assert!(!is_err);
    }

    #[test]
    fn tool_definitions_lists_three_tools() {
        let defs = tool_definitions();
        let tools = defs["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
    }
}
```

- [ ] **Step 3: Write `crates/snakewood-daemon/src/mcp/mod.rs`**

```rust
//! The MCP bridge: hand-rolled JSON-RPC over stdio, forwarding tool calls to the
//! daemon's command API. Synchronous; used by the `snakewood-mcp` binary.

pub mod protocol;
pub mod tools;

pub use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use tools::{response_to_text, tool_call_to_request, tool_definitions};
```

- [ ] **Step 4: Wire into `lib.rs`**

Add to `crates/snakewood-daemon/src/lib.rs`:
```rust
pub mod mcp;
```

- [ ] **Step 5: Run tests + fmt + clippy**

Run: `cargo test -p snakewood-daemon mcp` (all pass), then `cargo fmt -p snakewood-daemon && cargo fmt -p snakewood-daemon -- --check && cargo clippy -p snakewood-daemon --all-targets -- -D warnings`.

- [ ] **Step 6: Commit**

```bash
git add crates/snakewood-daemon/src/mcp crates/snakewood-daemon/src/lib.rs
git commit -m "feat: add MCP JSON-RPC types and tool<->API mapping"
```

---

### Task 3: `DaemonClient` trait + `dispatch_rpc`

**Files:**
- Create: `crates/snakewood-daemon/src/mcp/dispatch.rs`
- Modify: `crates/snakewood-daemon/src/mcp/mod.rs`

**Interfaces:**
- Consumes: `mcp::{JsonRpcRequest, JsonRpcResponse, tool_definitions, tool_call_to_request, response_to_text}`; `api::{ApiRequest, ApiResponse}`; `serde_json::{json, Value}`.
- Produces:
  - `pub trait DaemonClient { fn request(&mut self, req: ApiRequest) -> std::io::Result<ApiResponse>; }`.
  - `pub fn dispatch_rpc(req: &JsonRpcRequest, session: u64, client: &mut dyn DaemonClient) -> Option<JsonRpcResponse>` — `initialize` → server info result; `notifications/initialized` (and any `notifications/*`) → `None` (no response); `tools/list` → the tool defs; `tools/call` → map to `ApiRequest`, `client.request(...)`, format the result as MCP content; unknown method → JSON-RPC error `-32601`. Requests with no `id` (notifications) return `None`.

- [ ] **Step 1: Write `crates/snakewood-daemon/src/mcp/dispatch.rs` with tests**

```rust
use serde_json::{json, Value};

use crate::api::{ApiRequest, ApiResponse};
use crate::mcp::{response_to_text, tool_call_to_request, tool_definitions, JsonRpcRequest, JsonRpcResponse};

/// A transport to the daemon command API (real: TCP; test: mock).
pub trait DaemonClient {
    fn request(&mut self, req: ApiRequest) -> std::io::Result<ApiResponse>;
}

/// Handle one JSON-RPC request. Returns `None` for notifications (no reply).
pub fn dispatch_rpc(
    req: &JsonRpcRequest,
    session: u64,
    client: &mut dyn DaemonClient,
) -> Option<JsonRpcResponse> {
    // Notifications (no id) get no response.
    let id = req.id.clone()?;

    match req.method.as_str() {
        "initialize" => Some(JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "snakewood", "version": "0.1.0" }
            }),
        )),
        "tools/list" => Some(JsonRpcResponse::ok(id, tool_definitions())),
        "tools/call" => {
            let params = req.params.clone().unwrap_or(Value::Null);
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let empty = json!({});
            let args = params.get("arguments").unwrap_or(&empty);
            let api_req = match tool_call_to_request(name, args, session) {
                Ok(r) => r,
                Err(e) => return Some(tool_result(id, &e, true)),
            };
            match client.request(api_req) {
                Ok(resp) => {
                    let (text, is_err) = response_to_text(&resp);
                    Some(tool_result(id, &text, is_err))
                }
                Err(e) => Some(tool_result(id, &format!("daemon error: {e}"), true)),
            }
        }
        _ => Some(JsonRpcResponse::error(id, -32601, format!("method not found: {}", req.method))),
    }
}

/// Build an MCP `tools/call` result payload.
fn tool_result(id: Value, text: &str, is_error: bool) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        json!({
            "content": [ { "type": "text", "text": text } ],
            "isError": is_error
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use snakewood_core::{Direction, PresentationNode};

    // A mock daemon: records the last request, returns a canned response.
    struct MockClient {
        last: Option<ApiRequest>,
        reply: ApiResponse,
    }
    impl DaemonClient for MockClient {
        fn request(&mut self, req: ApiRequest) -> std::io::Result<ApiResponse> {
            self.last = Some(req);
            Ok(self.reply.clone())
        }
    }

    fn rpc(method: &str, id: Option<i64>, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: id.map(Value::from),
            method: method.to_string(),
            params: Some(params),
        }
    }

    #[test]
    fn initialize_returns_server_info() {
        let mut client = MockClient { last: None, reply: ApiResponse::Ok { messages: vec![] } };
        let resp = dispatch_rpc(&rpc("initialize", Some(1), Value::Null), 0, &mut client).unwrap();
        let v = resp.result.unwrap();
        assert_eq!(v["serverInfo"]["name"], "snakewood");
        assert_eq!(v["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn notification_yields_no_response() {
        let mut client = MockClient { last: None, reply: ApiResponse::Ok { messages: vec![] } };
        let n = JsonRpcRequest { jsonrpc: "2.0".to_string(), id: None, method: "notifications/initialized".to_string(), params: None };
        assert!(dispatch_rpc(&n, 0, &mut client).is_none());
    }

    #[test]
    fn tools_list_returns_three() {
        let mut client = MockClient { last: None, reply: ApiResponse::Ok { messages: vec![] } };
        let resp = dispatch_rpc(&rpc("tools/list", Some(2), Value::Null), 0, &mut client).unwrap();
        assert_eq!(resp.result.unwrap()["tools"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn tools_call_move_forwards_to_daemon_and_renders() {
        let mut client = MockClient {
            last: None,
            reply: ApiResponse::Ok { messages: vec![PresentationNode::RoomName("The Old Well".to_string())] },
        };
        let params = json!({"name": "snakewood_move", "arguments": {"direction": "north"}});
        let resp = dispatch_rpc(&rpc("tools/call", Some(3), params), 7, &mut client).unwrap();
        // Forwarded the right ApiRequest (with our session).
        assert_eq!(client.last, Some(ApiRequest::Move { session: 7, direction: Direction::North }));
        // Rendered the daemon's view into the tool content.
        let v = resp.result.unwrap();
        assert_eq!(v["isError"], false);
        assert!(v["content"][0]["text"].as_str().unwrap().contains("The Old Well"));
    }

    #[test]
    fn tools_call_bad_direction_is_tool_error_not_forwarded() {
        let mut client = MockClient { last: None, reply: ApiResponse::Ok { messages: vec![] } };
        let params = json!({"name": "snakewood_move", "arguments": {"direction": "sideways"}});
        let resp = dispatch_rpc(&rpc("tools/call", Some(4), params), 0, &mut client).unwrap();
        assert!(client.last.is_none()); // never reached the daemon
        assert_eq!(resp.result.unwrap()["isError"], true);
    }

    #[test]
    fn unknown_method_is_jsonrpc_error() {
        let mut client = MockClient { last: None, reply: ApiResponse::Ok { messages: vec![] } };
        let resp = dispatch_rpc(&rpc("frobnicate", Some(5), Value::Null), 0, &mut client).unwrap();
        assert_eq!(resp.error.unwrap().code, -32601);
    }
}
```

- [ ] **Step 2: Wire into `mcp/mod.rs`**

Add:
```rust
pub mod dispatch;

pub use dispatch::{dispatch_rpc, DaemonClient};
```

- [ ] **Step 3: Run tests + fmt + clippy**

Run: `cargo test -p snakewood-daemon mcp::dispatch` (6 tests pass), then fmt + clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-daemon/src/mcp/dispatch.rs crates/snakewood-daemon/src/mcp/mod.rs
git commit -m "feat: add dispatch_rpc MCP method handler over a DaemonClient trait"
```

---

### Task 4: `TcpDaemonClient` (sync TCP client with reconnect)

**Files:**
- Create: `crates/snakewood-daemon/src/mcp/client.rs`
- Modify: `crates/snakewood-daemon/src/mcp/mod.rs`

**Interfaces:**
- Consumes: `std::net::TcpStream`, `std::io::{BufRead, BufReader, Write}`; `api::{ApiRequest, ApiResponse}`; `mcp::DaemonClient`.
- Produces:
  - `pub struct TcpDaemonClient { addr: String, builder: String, stream: Option<...>, pub session: u64 }`.
  - `TcpDaemonClient::connect(addr: &str, builder: &str) -> std::io::Result<TcpDaemonClient>` — opens the socket, sends `ConnectAs { actor: builder }`, reads the `Connected { session }`, stores the session. Returns the client.
  - `impl DaemonClient for TcpDaemonClient` — `request` writes the JSON line + `\n`, reads one response line, deserializes. On an I/O error, tries ONE reconnect (re-`ConnectAs`) and retries; the reconnect updates `self.session`.

- [ ] **Step 1: Write `crates/snakewood-daemon/src/mcp/client.rs`**

```rust
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

use crate::api::{ApiRequest, ApiResponse};
use crate::mcp::DaemonClient;

/// A synchronous line-delimited-JSON client to the daemon command API, holding a
/// persistent connection bound to a named builder actor, reconnecting on failure.
pub struct TcpDaemonClient {
    addr: String,
    builder: String,
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    pub session: u64,
}

fn send_line(writer: &mut TcpStream, req: &ApiRequest) -> std::io::Result<()> {
    let mut line = serde_json::to_string(req).map_err(to_io)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

fn read_response(reader: &mut BufReader<TcpStream>) -> std::io::Result<ApiResponse> {
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "daemon closed"));
    }
    serde_json::from_str(line.trim_end()).map_err(to_io)
}

fn to_io<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
}

impl TcpDaemonClient {
    /// Connect and attach to the named builder; returns the client with its session.
    pub fn connect(addr: &str, builder: &str) -> std::io::Result<TcpDaemonClient> {
        let stream = TcpStream::connect(addr)?;
        let writer = stream.try_clone()?;
        let mut reader = BufReader::new(stream);
        let mut writer2 = writer;
        send_line(&mut writer2, &ApiRequest::ConnectAs { actor: builder.to_string() })?;
        let session = match read_response(&mut reader)? {
            ApiResponse::Connected { session, .. } => session,
            other => return Err(to_io(format!("expected Connected, got {other:?}"))),
        };
        Ok(TcpDaemonClient {
            addr: addr.to_string(),
            builder: builder.to_string(),
            reader,
            writer: writer2,
            session,
        })
    }

    fn reconnect(&mut self) -> std::io::Result<()> {
        let fresh = TcpDaemonClient::connect(&self.addr, &self.builder)?;
        self.reader = fresh.reader;
        self.writer = fresh.writer;
        self.session = fresh.session;
        Ok(())
    }
}

impl DaemonClient for TcpDaemonClient {
    fn request(&mut self, req: ApiRequest) -> std::io::Result<ApiResponse> {
        // Try once; on I/O failure, reconnect (re-ConnectAs) and retry once.
        match send_line(&mut self.writer, &req).and_then(|()| read_response(&mut self.reader)) {
            Ok(resp) => Ok(resp),
            Err(_) => {
                self.reconnect()?;
                // Re-issue with the (possibly new) session id patched in for
                // session-scoped requests.
                let req = with_session(req, self.session);
                send_line(&mut self.writer, &req)?;
                read_response(&mut self.reader)
            }
        }
    }
}

/// Replace the session id in a session-scoped request (used after reconnect).
fn with_session(req: ApiRequest, session: u64) -> ApiRequest {
    match req {
        ApiRequest::Look { .. } => ApiRequest::Look { session },
        ApiRequest::Move { direction, .. } => ApiRequest::Move { session, direction },
        ApiRequest::Dig { direction, id, name, description, .. } => {
            ApiRequest::Dig { session, direction, id, name, description }
        }
        ApiRequest::Disconnect { .. } => ApiRequest::Disconnect { session },
        other => other, // Connect / ConnectAs carry no session
    }
}
```

- [ ] **Step 2: Wire into `mcp/mod.rs`**

Add:
```rust
pub mod client;

pub use client::TcpDaemonClient;
```

- [ ] **Step 3: Build + fmt + clippy**

Run: `cargo build -p snakewood-daemon`, then `cargo fmt -p snakewood-daemon && cargo fmt -p snakewood-daemon -- --check && cargo clippy -p snakewood-daemon --all-targets -- -D warnings`. (No unit test here — exercised by the Task 6 integration test against a real daemon.)

- [ ] **Step 4: Commit**

```bash
git add crates/snakewood-daemon/src/mcp/client.rs crates/snakewood-daemon/src/mcp/mod.rs
git commit -m "feat: add TcpDaemonClient (sync API client with reconnect)"
```

---

### Task 5: The `snakewood-mcp` binary (stdio loop)

**Files:**
- Create: `crates/snakewood-daemon/src/bin/snakewood-mcp.rs`

**Interfaces:**
- Consumes: `snakewood_daemon::mcp::{dispatch_rpc, JsonRpcRequest, TcpDaemonClient, DaemonClient}`; `std::io`.
- Produces: a binary that connects a `TcpDaemonClient` to `SNAKEWOOD_API_ADDR` (default `127.0.0.1:4001`) as builder `player/mcp-builder`, then reads JSON-RPC lines from stdin, dispatches each via `dispatch_rpc` (using the client's `session`), and writes each non-`None` response as a JSON line to stdout (flushed).

- [ ] **Step 1: Write `crates/snakewood-daemon/src/bin/snakewood-mcp.rs`**

```rust
//! snakewood-mcp: an MCP (JSON-RPC over stdio) bridge to the snakewood daemon.
//! Reconnecting thin client — restart it freely without disturbing the world.

use std::io::{BufRead, Write};

use snakewood_daemon::mcp::{dispatch_rpc, JsonRpcRequest, TcpDaemonClient};

fn main() -> std::io::Result<()> {
    let addr = std::env::var("SNAKEWOOD_API_ADDR").unwrap_or_else(|_| "127.0.0.1:4001".to_string());
    let builder = std::env::var("SNAKEWOOD_MCP_ACTOR").unwrap_or_else(|_| "player/mcp-builder".to_string());

    let mut client = TcpDaemonClient::connect(&addr, &builder)?;
    eprintln!("snakewood-mcp connected to {addr} as {builder} (session {})", client.session);

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("snakewood-mcp: bad JSON-RPC line: {e}");
                continue;
            }
        };
        let session = client.session;
        if let Some(resp) = dispatch_rpc(&req, session, &mut client) {
            let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| {
                "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"serialize failed\"}}".to_string()
            });
            out.push('\n');
            stdout.write_all(out.as_bytes())?;
            stdout.flush()?;
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Build + fmt + clippy**

Run: `cargo build -p snakewood-daemon --bin snakewood-mcp` (compiles), then `cargo fmt -p snakewood-daemon && cargo fmt -p snakewood-daemon -- --check && cargo clippy -p snakewood-daemon --all-targets -- -D warnings`.

- [ ] **Step 3: Commit**

```bash
git add crates/snakewood-daemon/src/bin/snakewood-mcp.rs
git commit -m "feat: add snakewood-mcp binary (MCP stdio bridge)"
```

---

### Task 6: Integration test — MCP dispatch against a real daemon

**Files:**
- Create: `crates/snakewood-daemon/tests/mcp_bridge.rs`

**Interfaces:**
- Consumes: `snakewood_daemon::{Engine, ManualClock}`, `snakewood_daemon::api::serve_api`, `snakewood_daemon::mcp::{TcpDaemonClient, DaemonClient, dispatch_rpc, JsonRpcRequest}`; `snakewood_core::*`; tokio (dev — already available); `serde_json`.
- Produces: an integration test that runs the daemon `serve_api` on an ephemeral port (on a background thread with a current-thread tokio runtime + LocalSet), then — from the test thread — creates a real `TcpDaemonClient` (which `ConnectAs`-es the builder), and drives `dispatch_rpc` for `initialize`, `tools/list`, and `tools/call` (`snakewood_dig` then `snakewood_move`), asserting the MCP results.

- [ ] **Step 1: Write `crates/snakewood-daemon/tests/mcp_bridge.rs`**

```rust
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use serde_json::{json, Value};
use snakewood_core::{Direction, EntityId, Realm, Room, World};
use snakewood_daemon::api::serve_api;
use snakewood_daemon::mcp::{dispatch_rpc, DaemonClient, JsonRpcRequest, TcpDaemonClient};
use snakewood_daemon::{Engine, ManualClock};

fn id(s: &str) -> EntityId {
    EntityId::new(s).unwrap()
}

fn two_room_world() -> World {
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

fn rpc(method: &str, id_num: i64, params: Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(Value::from(id_num)),
        method: method.to_string(),
        params: Some(params),
    }
}

#[test]
fn mcp_bridge_drives_the_daemon() {
    // Start the daemon API on a background thread with its own current-thread runtime.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            let engine = Rc::new(RefCell::new(Engine::new(
                Realm::new(two_room_world()),
                Box::new(ManualClock::new(0)),
            )));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            serve_api(listener, engine, id("snakewood/clearing")).await;
        });
    });
    let addr = rx.recv().unwrap().to_string();

    // Bridge client connects as the persistent builder.
    let mut client = TcpDaemonClient::connect(&addr, "player/mcp-builder").unwrap();

    // initialize
    let init = dispatch_rpc(&rpc("initialize", 1, Value::Null), client.session, &mut client).unwrap();
    assert_eq!(init.result.unwrap()["serverInfo"]["name"], "snakewood");

    // tools/list
    let list = dispatch_rpc(&rpc("tools/list", 2, Value::Null), client.session, &mut client).unwrap();
    assert_eq!(list.result.unwrap()["tools"].as_array().unwrap().len(), 3);

    // tools/call snakewood_dig east
    let dig = dispatch_rpc(
        &rpc("tools/call", 3, json!({
            "name": "snakewood_dig",
            "arguments": {"direction":"east","id":"snakewood/hollow","name":"A Hollow","description":"Mossy."}
        })),
        client.session,
        &mut client,
    ).unwrap();
    let dig_text = dig.result.unwrap()["content"][0]["text"].as_str().unwrap().to_string();
    assert!(dig_text.contains("east"), "dig view should list the new east exit: {dig_text}");

    // tools/call snakewood_move east -> into the dug room
    let mv = dispatch_rpc(
        &rpc("tools/call", 4, json!({"name": "snakewood_move", "arguments": {"direction":"east"}})),
        client.session,
        &mut client,
    ).unwrap();
    let mv_text = mv.result.unwrap()["content"][0]["text"].as_str().unwrap().to_string();
    assert!(mv_text.contains("A Hollow"), "move should arrive at the dug room: {mv_text}");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p snakewood-daemon --test mcp_bridge`
Expected: PASS (`mcp_bridge_drives_the_daemon`). This proves the full path: MCP JSON-RPC → tool mapping → TcpDaemonClient → daemon API → dig/move → rendered MCP content. If it hangs, the daemon thread's `serve_api` may not have bound before the client connected — but the `mpsc` handshake (send addr after bind) prevents that.

- [ ] **Step 3: fmt + clippy, commit**

Run: `cargo fmt -p snakewood-daemon && cargo fmt -p snakewood-daemon -- --check && cargo clippy -p snakewood-daemon --all-targets -- -D warnings`. Then:
```bash
git add crates/snakewood-daemon/tests/mcp_bridge.rs
git commit -m "test: MCP bridge drives dig+move against a real daemon end-to-end"
```

---

### Task 7: Stage + M1 completion verification

**Files:** none (verification only).

- [ ] **Step 1: Full workspace suite**

Run: `cargo test --workspace`
Expected: all pass — core + daemon (engine/api/telnet/mcp + all integration tests incl. `mcp_bridge`, `api_e2e`, `telnet_e2e`, `persistence_restart`, `engine_goblin`).

- [ ] **Step 2: Clippy + fmt**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no diffs. If diffs, `cargo fmt`, re-run, commit.

- [ ] **Step 3: Manual MCP smoke — drive the bridge binary over stdio**

Start the daemon, then pipe JSON-RPC to the bridge's stdin (the bridge connects to the daemon API):
```bash
DATA="$(mktemp -d)/world"
SNAKEWOOD_DATA="$DATA" SNAKEWOOD_ADDR=127.0.0.1:4200 SNAKEWOOD_API_ADDR=127.0.0.1:4201 cargo run -p snakewood-daemon >/tmp/sw_daemon.log 2>&1 &
DPID=$!
sleep 4
printf '{"jsonrpc":"2.0","id":1,"method":"initialize"}\n{"jsonrpc":"2.0","id":2,"method":"tools/list"}\n{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"snakewood_look","arguments":{}}}\n{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"snakewood_dig","arguments":{"direction":"east","id":"snakewood/hollow","name":"A Hollow","description":"Mossy."}}}\n' \
  | SNAKEWOOD_API_ADDR=127.0.0.1:4201 cargo run -p snakewood-daemon --bin snakewood-mcp > /tmp/sw_mcp.out 2>/tmp/sw_mcp.err || true
kill "$DPID" 2>/dev/null || true; wait "$DPID" 2>/dev/null || true
echo "=== MCP stdout ==="; cat /tmp/sw_mcp.out
```
Expected: four JSON-RPC response lines — an `initialize` result with `serverInfo.name = "snakewood"`, a `tools/list` with 3 tools, a `tools/call` look result containing "Snakewood Clearing", and a `dig` result whose text lists an `east` exit. (Skip if the environment can't run two processes; `mcp_bridge` already proves the path.)

- [ ] **Step 4: Commit any fmt fixes; then the plan is done**

```bash
git add -A
git commit -m "chore: stage 3d-mcp verification — clippy clean, cargo fmt, workspace green"
```
(Skip if nothing to commit.) **M1 (the walking skeleton) is complete once this stage merges.**

---

## Self-Review

**1. Spec coverage (spec §2 MCP as first-class reconnecting client; §7 M1 MCP criteria):**
- MCP server (JSON-RPC over stdio) exposing IC `look`/`move` (structured→rendered) + OOC `dig` — Tasks 2–5. ✓
- Thin reconnecting client to the persistent daemon over the command socket — Tasks 4, 5 (`TcpDaemonClient` reconnect + re-`ConnectAs`). ✓
- Restart-decoupling / stable identity: `ConnectAs` + persistent builder mob (session-only disconnect) — Task 1. ✓
- End-to-end proof (dig+move via MCP against a real daemon) — Task 6. ✓
- Correctly deferred: MCP resources/prompts, auth, multi-builder, non-stdio transport; real Claude-Code-connect is a manual post-merge config step. ✓

**2. Placeholder scan:** No "TBD/TODO". The manual smoke (Task 7 Step 3) is explicitly optional. No labeled false-starts this plan.

**3. Type consistency:** `ApiRequest::ConnectAs { actor: String }` (Task 1) is produced by `TcpDaemonClient::connect` (Task 4) and consumed by `handle_api_request` (Task 1). `DaemonClient::request(&mut self, ApiRequest) -> io::Result<ApiResponse>` is implemented by `TcpDaemonClient` (Task 4) and the test `MockClient` (Task 3), and consumed by `dispatch_rpc` (Task 3). `JsonRpcRequest`/`JsonRpcResponse` (Task 2) flow through `dispatch_rpc` (Task 3) and the bin (Task 5). `tool_call_to_request(&str, &Value, u64) -> Result<ApiRequest,String>` and `response_to_text(&ApiResponse) -> (String, bool)` (Task 2) used by `dispatch_rpc` (Task 3). `attach_named(&mut Engine, &EntityId, &EntityId) -> SessionId` (Task 1) used by the handler. `TcpDaemonClient.session: u64` fed as the `session` arg to `dispatch_rpc` (Tasks 5, 6). The bridge is sync (`std::net`) while the daemon test spins the async `serve_api` on a background thread — no tokio in the bridge itself. ✓
