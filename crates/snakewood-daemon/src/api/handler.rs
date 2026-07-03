use snakewood_core::{EntityId, Intent};

use crate::api::{ApiRequest, ApiResponse};
use crate::telnet::{attach_named, despawn_player, spawn_player};
use crate::{Engine, SessionId};

/// Look up the actor bound to a session, or produce an Error response.
fn actor_of(engine: &Engine, session: u64) -> Result<EntityId, ApiResponse> {
    match engine.session_actor(SessionId(session)) {
        Some(actor) => Ok(actor.clone()),
        None => Err(ApiResponse::Error {
            message: format!("unknown session {session}"),
        }),
    }
}

/// Dispatch a structured API request against the engine.
pub fn handle_api_request(
    engine: &mut Engine,
    req: ApiRequest,
    start_room: &EntityId,
) -> ApiResponse {
    match req {
        ApiRequest::Connect => {
            let (sid, actor) = spawn_player(engine, start_room);
            engine.submit(
                sid,
                Intent::Look {
                    actor: actor.clone(),
                },
            );
            let view = engine.poll(sid);
            ApiResponse::Connected {
                session: sid.0,
                actor: actor.to_string(),
                view,
            }
        }
        ApiRequest::ConnectAs { actor } => {
            let actor_id = match EntityId::new(actor.clone()) {
                Ok(id) => id,
                Err(_) => {
                    return ApiResponse::Error {
                        message: format!("invalid actor id: {actor}"),
                    }
                }
            };
            let sid = attach_named(engine, &actor_id, start_room);
            engine.submit(
                sid,
                Intent::Look {
                    actor: actor_id.clone(),
                },
            );
            let view = engine.poll(sid);
            ApiResponse::Connected {
                session: sid.0,
                actor: actor_id.to_string(),
                view,
            }
        }
        ApiRequest::Look { session } => {
            let actor = match actor_of(engine, session) {
                Ok(a) => a,
                Err(e) => return e,
            };
            engine.submit(SessionId(session), Intent::Look { actor });
            ApiResponse::Ok {
                messages: engine.poll(SessionId(session)),
            }
        }
        ApiRequest::Move { session, direction } => {
            let actor = match actor_of(engine, session) {
                Ok(a) => a,
                Err(e) => return e,
            };
            engine.submit(SessionId(session), Intent::Move { actor, direction });
            ApiResponse::Ok {
                messages: engine.poll(SessionId(session)),
            }
        }
        ApiRequest::Dig {
            session,
            direction,
            id,
            name,
            description,
        } => {
            match engine.dig(SessionId(session), direction, &id, &name, &description) {
                Ok(_) => {
                    // Show the updated room so the client sees the new exit.
                    if let Some(actor) = engine.session_actor(SessionId(session)).cloned() {
                        engine.submit(SessionId(session), Intent::Look { actor });
                        ApiResponse::Ok {
                            messages: engine.poll(SessionId(session)),
                        }
                    } else {
                        ApiResponse::Ok {
                            messages: Vec::new(),
                        }
                    }
                }
                Err(e) => ApiResponse::Error {
                    message: format!("dig failed: {e:?}"),
                },
            }
        }
        ApiRequest::Disconnect { session } => {
            if let Some(actor) = engine.session_actor(SessionId(session)).cloned() {
                despawn_player(engine, SessionId(session), &actor);
            }
            ApiResponse::Ok {
                messages: Vec::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualClock;
    use snakewood_core::{Direction, PresentationNode, Realm};

    // Reuse the engine test helper's two-room world by rebuilding it here.
    fn engine() -> Engine {
        use snakewood_core::{Room, World};
        use std::collections::BTreeMap;
        let mut exits = BTreeMap::new();
        exits.insert(
            Direction::North,
            EntityId::new("snakewood/old-well").unwrap(),
        );
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

    #[test]
    fn connect_returns_session_and_start_room_view() {
        let mut e = engine();
        let resp = handle_api_request(&mut e, ApiRequest::Connect, &start());
        match resp {
            ApiResponse::Connected {
                session,
                actor,
                view,
            } => {
                assert_eq!(actor, "player/anon-0");
                assert_eq!(session, 0);
                assert!(view.contains(&PresentationNode::RoomName(
                    "Snakewood Clearing".to_string()
                )));
            }
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[test]
    fn connect_as_attaches_named_builder() {
        let mut e = engine();
        let resp = handle_api_request(
            &mut e,
            ApiRequest::ConnectAs {
                actor: "player/mcp-builder".to_string(),
            },
            &start(),
        );
        match resp {
            ApiResponse::Connected { actor, view, .. } => {
                assert_eq!(actor, "player/mcp-builder");
                assert!(view.contains(&PresentationNode::RoomName(
                    "Snakewood Clearing".to_string()
                )));
            }
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[test]
    fn move_returns_new_room_view() {
        let mut e = engine();
        let ApiResponse::Connected { session, .. } =
            handle_api_request(&mut e, ApiRequest::Connect, &start())
        else {
            panic!("connect failed");
        };
        let resp = handle_api_request(
            &mut e,
            ApiRequest::Move {
                session,
                direction: Direction::North,
            },
            &start(),
        );
        match resp {
            ApiResponse::Ok { messages } => {
                assert!(messages.contains(&PresentationNode::RoomName("The Old Well".to_string())));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn dig_then_look_shows_new_exit() {
        let mut e = engine();
        let ApiResponse::Connected { session, .. } =
            handle_api_request(&mut e, ApiRequest::Connect, &start())
        else {
            panic!("connect failed");
        };
        let resp = handle_api_request(
            &mut e,
            ApiRequest::Dig {
                session,
                direction: Direction::East,
                id: "snakewood/hollow".to_string(),
                name: "A Hollow".to_string(),
                description: "Mossy.".to_string(),
            },
            &start(),
        );
        match resp {
            ApiResponse::Ok { messages } => {
                // The clearing view now lists an east exit.
                assert!(messages.iter().any(|n| matches!(n, PresentationNode::Exits(dirs) if dirs.contains(&Direction::East))));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn unknown_session_is_error() {
        let mut e = engine();
        let resp = handle_api_request(&mut e, ApiRequest::Look { session: 999 }, &start());
        assert!(matches!(resp, ApiResponse::Error { .. }));
    }

    #[test]
    fn connect_ids_are_distinct_across_calls() {
        // Guards Fix A: two Connect requests on the same engine must never
        // collide on the same anon id (this is what let telnet and API
        // players stomp on each other before the fix).
        let mut e = engine();
        let ApiResponse::Connected { actor: actor1, .. } =
            handle_api_request(&mut e, ApiRequest::Connect, &start())
        else {
            panic!("connect failed");
        };
        let ApiResponse::Connected { actor: actor2, .. } =
            handle_api_request(&mut e, ApiRequest::Connect, &start())
        else {
            panic!("connect failed");
        };
        assert_eq!(actor1, "player/anon-0");
        assert_eq!(actor2, "player/anon-1");
        assert_ne!(actor1, actor2);
    }
}
