use serde::{Deserialize, Serialize};

use snakewood_core::{Direction, PresentationNode};

/// A structured command from an API client (e.g. the MCP bridge).
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ApiRequest {
    Connect,
    ConnectAs {
        actor: String,
    },
    Look {
        session: u64,
    },
    Move {
        session: u64,
        direction: Direction,
    },
    Dig {
        session: u64,
        direction: Direction,
        id: String,
        name: String,
        description: String,
    },
    Disconnect {
        session: u64,
    },
}

/// A structured response to an API client.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApiResponse {
    Connected {
        session: u64,
        actor: String,
        view: Vec<PresentationNode>,
    },
    Ok {
        messages: Vec<PresentationNode>,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_json_round_trips() {
        let req = ApiRequest::Move {
            session: 3,
            direction: Direction::North,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"op\":\"move\""));
        let back: ApiRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn connect_as_round_trips() {
        let req = ApiRequest::ConnectAs {
            actor: "player/mcp-builder".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"op\":\"connect_as\""));
        let back: ApiRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn dig_request_parses_from_json() {
        let json = r#"{"op":"dig","session":1,"direction":"East","id":"snakewood/hollow","name":"A Hollow","description":"Mossy."}"#;
        let req: ApiRequest = serde_json::from_str(json).unwrap();
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
    fn response_json_round_trips() {
        let resp = ApiResponse::Ok {
            messages: vec![PresentationNode::Line(snakewood_core::plain_text("hi"))],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"status\":\"ok\""));
        let back: ApiResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, resp);
    }
}
