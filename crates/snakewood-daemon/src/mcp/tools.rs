use serde_json::{json, Value};

use snakewood_core::Direction;

use crate::api::{ApiRequest, ApiResponse};
use crate::telnet::{render, RenderStyle};

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
        let s = args
            .get("direction")
            .and_then(|v| v.as_str())
            .ok_or("missing 'direction'")?;
        parse_direction(s).ok_or_else(|| format!("bad direction: {s}"))
    };
    match name {
        "snakewood_look" => Ok(ApiRequest::Look { session }),
        "snakewood_move" => Ok(ApiRequest::Move {
            session,
            direction: dir(args)?,
        }),
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
        ApiResponse::Connected { view, .. } => (render(view, RenderStyle::Plain), false),
        ApiResponse::Ok { messages } => {
            let text = render(messages, RenderStyle::Plain);
            (
                if text.is_empty() {
                    "OK".to_string()
                } else {
                    text
                },
                false,
            )
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
        let req =
            tool_call_to_request("snakewood_move", &json!({"direction": "north"}), 5).unwrap();
        assert_eq!(
            req,
            ApiRequest::Move {
                session: 5,
                direction: Direction::North
            }
        );
    }

    #[test]
    fn dig_tool_maps_all_fields() {
        let args = json!({"direction":"east","id":"snakewood/hollow","name":"A Hollow","description":"Mossy."});
        let req = tool_call_to_request("snakewood_dig", &args, 1).unwrap();
        assert_eq!(
            req,
            ApiRequest::Dig {
                session: 1,
                direction: Direction::East,
                id: "snakewood/hollow".to_string(),
                name: "A Hollow".to_string(),
                description: "Mossy.".to_string(),
            }
        );
    }

    #[test]
    fn unknown_tool_and_bad_direction_error() {
        assert!(tool_call_to_request("nope", &json!({}), 0).is_err());
        assert!(
            tool_call_to_request("snakewood_move", &json!({"direction":"sideways"}), 0).is_err()
        );
    }

    #[test]
    fn response_text_marks_errors() {
        let (text, is_err) = response_to_text(&ApiResponse::Error {
            message: "boom".to_string(),
        });
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
