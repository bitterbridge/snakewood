use serde_json::{json, Value};

use crate::api::{ApiRequest, ApiResponse};
use crate::mcp::{
    response_to_text, tool_call_to_request, tool_definitions, JsonRpcRequest, JsonRpcResponse,
};

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
        _ => Some(JsonRpcResponse::error(
            id,
            -32601,
            format!("method not found: {}", req.method),
        )),
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
        let mut client = MockClient {
            last: None,
            reply: ApiResponse::Ok { messages: vec![] },
        };
        let resp = dispatch_rpc(&rpc("initialize", Some(1), Value::Null), 0, &mut client).unwrap();
        let v = resp.result.unwrap();
        assert_eq!(v["serverInfo"]["name"], "snakewood");
        assert_eq!(v["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn notification_yields_no_response() {
        let mut client = MockClient {
            last: None,
            reply: ApiResponse::Ok { messages: vec![] },
        };
        let n = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: "notifications/initialized".to_string(),
            params: None,
        };
        assert!(dispatch_rpc(&n, 0, &mut client).is_none());
    }

    #[test]
    fn tools_list_returns_three() {
        let mut client = MockClient {
            last: None,
            reply: ApiResponse::Ok { messages: vec![] },
        };
        let resp = dispatch_rpc(&rpc("tools/list", Some(2), Value::Null), 0, &mut client).unwrap();
        assert_eq!(resp.result.unwrap()["tools"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn tools_call_move_forwards_to_daemon_and_renders() {
        let mut client = MockClient {
            last: None,
            reply: ApiResponse::Ok {
                messages: vec![PresentationNode::RoomName("The Old Well".to_string())],
            },
        };
        let params = json!({"name": "snakewood_move", "arguments": {"direction": "north"}});
        let resp = dispatch_rpc(&rpc("tools/call", Some(3), params), 7, &mut client).unwrap();
        // Forwarded the right ApiRequest (with our session).
        assert_eq!(
            client.last,
            Some(ApiRequest::Move {
                session: 7,
                direction: Direction::North
            })
        );
        // Rendered the daemon's view into the tool content.
        let v = resp.result.unwrap();
        assert_eq!(v["isError"], false);
        assert!(v["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("The Old Well"));
    }

    #[test]
    fn tools_call_bad_direction_is_tool_error_not_forwarded() {
        let mut client = MockClient {
            last: None,
            reply: ApiResponse::Ok { messages: vec![] },
        };
        let params = json!({"name": "snakewood_move", "arguments": {"direction": "sideways"}});
        let resp = dispatch_rpc(&rpc("tools/call", Some(4), params), 0, &mut client).unwrap();
        assert!(client.last.is_none()); // never reached the daemon
        assert_eq!(resp.result.unwrap()["isError"], true);
    }

    #[test]
    fn unknown_method_is_jsonrpc_error() {
        let mut client = MockClient {
            last: None,
            reply: ApiResponse::Ok { messages: vec![] },
        };
        let resp = dispatch_rpc(&rpc("frobnicate", Some(5), Value::Null), 0, &mut client).unwrap();
        assert_eq!(resp.error.unwrap().code, -32601);
    }
}
