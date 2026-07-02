use std::collections::BTreeMap;

use snakewood_core::{dispatch, EntityId, Intent, PresentationNode, Realm};

use crate::session::{Session, SessionId};
use crate::Clock;

/// The synchronous core of the daemon: owns the world, the clock, and sessions.
pub struct Engine {
    realm: Realm,
    clock: Box<dyn Clock>,
    sessions: BTreeMap<SessionId, Session>,
    next_session: u64,
    tick: u64,
}

impl Engine {
    pub fn new(realm: Realm, clock: Box<dyn Clock>) -> Engine {
        Engine {
            realm,
            clock,
            sessions: BTreeMap::new(),
            next_session: 0,
            tick: 0,
        }
    }

    /// Register a new session bound to `actor`; returns its id.
    pub fn connect(&mut self, actor: EntityId) -> SessionId {
        let id = SessionId(self.next_session);
        self.next_session += 1;
        self.sessions.insert(id, Session::new(actor));
        id
    }

    /// Remove a session, returning it if present.
    pub fn disconnect(&mut self, id: SessionId) -> Option<Session> {
        self.sessions.remove(&id)
    }

    pub fn session_actor(&self, id: SessionId) -> Option<&EntityId> {
        self.sessions.get(&id).map(|s| &s.actor)
    }

    pub fn realm(&self) -> &Realm {
        &self.realm
    }

    pub fn realm_mut(&mut self) -> &mut Realm {
        &mut self.realm
    }

    /// Dispatch `intent` and fan the resulting presentation out to sessions.
    ///
    /// No-op unless `id` is a live session AND the intent acts as that session's
    /// own actor — a session may only drive the actor it is bound to. This is the
    /// authorization seam the transports (telnet, MCP) rely on.
    pub fn submit(&mut self, id: SessionId, intent: Intent) {
        let authorized = matches!(self.sessions.get(&id), Some(s) if &s.actor == intent.actor());
        if !authorized {
            return;
        }
        let result = dispatch(&mut self.realm, intent);
        for (recipient, node) in result.messages {
            for session in self.sessions.values_mut() {
                if session.actor == recipient {
                    session.outbox.push(node.clone());
                }
            }
        }
    }

    /// Drain a session's pending presentation.
    pub fn poll(&mut self, id: SessionId) -> Vec<PresentationNode> {
        match self.sessions.get_mut(&id) {
            Some(session) => std::mem::take(&mut session.outbox),
            None => Vec::new(),
        }
    }

    /// Advance the logical tick counter by one; returns the new count.
    pub fn tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    /// Current injected time in Unix seconds.
    pub fn now_unix(&self) -> i64 {
        self.clock.now_unix()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualClock;
    use snakewood_core::{Direction, Flag, Intent, Mob, PresentationNode, Room, World};
    use std::collections::BTreeSet;

    fn engine() -> Engine {
        Engine::new(Realm::new(World::default()), Box::new(ManualClock::new(0)))
    }

    fn world_two_rooms() -> World {
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
        world
    }

    fn engine_with_actor() -> (Engine, SessionId, EntityId) {
        let mut realm = Realm::new(world_two_rooms());
        let actor = EntityId::new("snakewood/pc/nathan").unwrap();
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        realm.insert_mob(Mob {
            id: actor.clone(),
            name: "Nathan".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        });
        let mut e = Engine::new(realm, Box::new(ManualClock::new(0)));
        let sid = e.connect(actor.clone());
        (e, sid, actor)
    }

    #[test]
    fn connect_assigns_distinct_ids_and_binds_actor() {
        let mut e = engine();
        let a = EntityId::new("snakewood/pc/a").unwrap();
        let b = EntityId::new("snakewood/pc/b").unwrap();
        let sa = e.connect(a.clone());
        let sb = e.connect(b.clone());
        assert_ne!(sa, sb);
        assert_eq!(e.session_actor(sa), Some(&a));
        assert_eq!(e.session_actor(sb), Some(&b));
    }

    #[test]
    fn disconnect_removes_session() {
        let mut e = engine();
        let a = EntityId::new("snakewood/pc/a").unwrap();
        let sa = e.connect(a);
        assert!(e.disconnect(sa).is_some());
        assert_eq!(e.session_actor(sa), None);
        assert!(e.disconnect(sa).is_none());
    }

    #[test]
    fn submit_move_routes_arrival_view_to_session_and_relocates() {
        let (mut e, sid, actor) = engine_with_actor();
        e.submit(
            sid,
            Intent::Move {
                actor: actor.clone(),
                direction: Direction::North,
            },
        );
        // world state changed
        assert_eq!(
            e.realm().mob_location(&actor).map(|r| r.as_str()),
            Some("snakewood/old-well")
        );
        // arrival view delivered to the session
        let view = e.poll(sid);
        assert!(view.contains(&PresentationNode::RoomName("The Old Well".to_string())));
        // draining leaves the outbox empty
        assert!(e.poll(sid).is_empty());
    }

    #[test]
    fn submit_move_no_exit_routes_fallback_message() {
        let (mut e, sid, actor) = engine_with_actor();
        e.submit(
            sid,
            Intent::Move {
                actor,
                direction: Direction::South,
            },
        );
        let view = e.poll(sid);
        assert!(view.contains(&PresentationNode::Denied(
            "You see no exit in that direction.".to_string()
        )));
    }

    #[test]
    fn submit_on_unknown_session_is_noop() {
        let (mut e, _sid, actor) = engine_with_actor();
        e.submit(SessionId(999), Intent::Look { actor });
        assert!(e.poll(SessionId(999)).is_empty());
    }

    #[test]
    fn submit_ignores_intent_acting_as_a_different_actor() {
        // A session bound to "nathan" cannot drive some other actor.
        let (mut e, sid, _actor) = engine_with_actor();
        let other = EntityId::new("snakewood/pc/impostor").unwrap();
        e.submit(sid, Intent::Move { actor: other.clone(), direction: Direction::North });
        // The bound actor did not move, the foreign actor is untouched, and the
        // session received nothing.
        assert_eq!(
            e.realm().mob_location(&EntityId::new("snakewood/pc/nathan").unwrap()).map(|r| r.as_str()),
            Some("snakewood/clearing")
        );
        assert!(e.realm().mob_location(&other).is_none());
        assert!(e.poll(sid).is_empty());
    }

    #[test]
    fn tick_advances_counter() {
        let mut e = engine();
        assert_eq!(e.tick_count(), 0);
        assert_eq!(e.tick(), 1);
        assert_eq!(e.tick(), 2);
        assert_eq!(e.tick_count(), 2);
    }

    #[test]
    fn now_unix_reflects_injected_clock() {
        let clock = ManualClock::new(500);
        // Keep a raw pointer-free handle by advancing before moving into the engine.
        clock.advance(100); // now 600
        let e = Engine::new(Realm::new(World::default()), Box::new(clock));
        assert_eq!(e.now_unix(), 600);
    }
}
