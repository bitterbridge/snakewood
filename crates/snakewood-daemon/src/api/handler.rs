use snakewood_core::{EntityId, Intent, PresentationNode};

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
            engine.enqueue(
                sid,
                Intent::Look {
                    actor: actor.clone(),
                },
            );
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
            engine.enqueue(
                sid,
                Intent::Look {
                    actor: actor_id.clone(),
                },
            );
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

    /// Begin a request, drive one drain, and build the final response.
    fn run(e: &mut Engine, req: ApiRequest) -> ApiResponse {
        match handle_api_request(e, req, &start()) {
            ApiOutcome::Ready(r) => r,
            ApiOutcome::AwaitDrain {
                session,
                before,
                shape,
            } => {
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
        match run(
            &mut e,
            ApiRequest::ConnectAs {
                actor: "player/mcp-builder".to_string(),
            },
        ) {
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
        let ApiResponse::Connected { session, .. } = run(&mut e, ApiRequest::Connect) else {
            panic!("connect failed");
        };
        match run(
            &mut e,
            ApiRequest::Move {
                session,
                direction: Direction::North,
            },
        ) {
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
        match run(
            &mut e,
            ApiRequest::Dig {
                session,
                direction: Direction::East,
                id: "snakewood/hollow".to_string(),
                name: "A Hollow".to_string(),
                description: "Mossy.".to_string(),
            },
        ) {
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
        match handle_api_request(&mut e, ApiRequest::Look { session: 999 }, &start()) {
            ApiOutcome::Ready(ApiResponse::Error { .. }) => {}
            other => panic!("expected Ready(Error), got {other:?}"),
        }
    }

    #[test]
    fn connect_ids_are_distinct_across_calls() {
        // Guards Fix A: two Connect requests on the same engine must never
        // collide on the same anon id (this is what let telnet and API
        // players stomp on each other before the fix).
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
