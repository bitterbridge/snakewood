use std::collections::BTreeMap;

use snakewood_core::{EntityId, Realm};

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualClock;
    use snakewood_core::World;

    fn engine() -> Engine {
        Engine::new(Realm::new(World::default()), Box::new(ManualClock::new(0)))
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
}
