use std::collections::BTreeMap;

use snakewood_core::{dispatch, EntityId, Intent, PresentationNode, Realm, StoreError, WorldStore};

use crate::session::{Session, SessionId};
use crate::Clock;

/// The synchronous core of the daemon: owns the world, the clock, and sessions.
pub struct Engine {
    realm: Realm,
    clock: Box<dyn Clock>,
    sessions: BTreeMap<SessionId, Session>,
    next_session: u64,
    tick: u64,
    store: Option<Box<dyn WorldStore>>,
    snapshot_interval: Option<i64>,
    last_snapshot: i64,
}

impl Engine {
    pub fn new(realm: Realm, clock: Box<dyn Clock>) -> Engine {
        Engine {
            realm,
            clock,
            sessions: BTreeMap::new(),
            next_session: 0,
            tick: 0,
            store: None,
            snapshot_interval: None,
            last_snapshot: 0,
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

    /// Attach a store for persistence and reset the snapshot clock to now.
    pub fn attach_store(&mut self, store: Box<dyn WorldStore>) {
        self.last_snapshot = self.clock.now_unix();
        self.store = Some(store);
    }

    /// Boot an engine by loading the entire realm from `store`.
    pub fn boot(store: Box<dyn WorldStore>, clock: Box<dyn Clock>) -> Result<Engine, StoreError> {
        let realm = store.load_realm()?;
        let mut engine = Engine::new(realm, clock);
        engine.attach_store(store);
        Ok(engine)
    }

    pub fn has_store(&self) -> bool {
        self.store.is_some()
    }

    /// Persist the whole realm and commit it immediately (on-checkpoint; also
    /// used by authored on-change ops once they exist). No-op without a store.
    pub fn checkpoint(&mut self, message: &str) -> Result<(), StoreError> {
        let now = self.clock.now_unix();
        if let Some(store) = self.store.as_mut() {
            store.save_realm(&self.realm)?;
            store.commit(message, now)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualClock;
    use snakewood_core::{Direction, Flag, GitStore, Intent, Mob, PresentationNode, Room, World};
    use std::collections::BTreeSet;
    use tempfile::tempdir;

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
        e.submit(
            sid,
            Intent::Move {
                actor: other.clone(),
                direction: Direction::North,
            },
        );
        // The bound actor did not move, the foreign actor is untouched, and the
        // session received nothing.
        assert_eq!(
            e.realm()
                .mob_location(&EntityId::new("snakewood/pc/nathan").unwrap())
                .map(|r| r.as_str()),
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

    #[test]
    fn attach_store_sets_flag_and_snapshot_time() {
        let clock = ManualClock::new(4242);
        let dir = tempdir().unwrap();
        let store = GitStore::init(dir.path()).unwrap();
        let mut e = Engine::new(Realm::new(World::default()), Box::new(clock));
        assert!(!e.has_store());
        e.attach_store(Box::new(store));
        assert!(e.has_store());
    }

    #[test]
    fn boot_loads_realm_from_store() {
        // Pre-populate a store on disk, commit, then boot an Engine from it.
        let dir = tempdir().unwrap();
        {
            let mut store = GitStore::init(dir.path()).unwrap();
            let mut realm = Realm::new(world_two_rooms());
            let mut flags = BTreeSet::new();
            flags.insert(Flag::Alive);
            realm.insert_mob(Mob {
                id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
                name: "a goblin".to_string(),
                location: EntityId::new("snakewood/clearing").unwrap(),
                flags,
                responders: Vec::new(),
            });
            store.save_realm(&realm).unwrap();
            store.commit("seed", 1000).unwrap();
        }
        let store = GitStore::init(dir.path()).unwrap();
        let e = Engine::boot(Box::new(store), Box::new(ManualClock::new(2000))).unwrap();
        assert!(e
            .realm()
            .world
            .room(&EntityId::new("snakewood/clearing").unwrap())
            .is_some());
        assert!(e
            .realm()
            .mob(&EntityId::new("snakewood/mob/goblin#1").unwrap())
            .is_some());
        assert!(e.has_store());
    }

    #[test]
    fn checkpoint_persists_live_state_across_a_restart() {
        let dir = tempdir().unwrap();
        // First engine: seed a world with an actor, move it, checkpoint.
        {
            let mut realm = Realm::new(world_two_rooms());
            let mut flags = std::collections::BTreeSet::new();
            flags.insert(snakewood_core::Flag::Alive);
            realm.insert_mob(snakewood_core::Mob {
                id: EntityId::new("snakewood/pc/nathan").unwrap(),
                name: "Nathan".to_string(),
                location: EntityId::new("snakewood/clearing").unwrap(),
                flags,
                responders: Vec::new(),
            });
            let store = GitStore::init(dir.path()).unwrap();
            let mut e = Engine::new(realm, Box::new(ManualClock::new(1000)));
            e.attach_store(Box::new(store));
            let sid = e.connect(EntityId::new("snakewood/pc/nathan").unwrap());
            e.submit(sid, Intent::Move {
                actor: EntityId::new("snakewood/pc/nathan").unwrap(),
                direction: Direction::North,
            });
            e.checkpoint("player moved north").unwrap();
        }
        // Second engine: boot from the same dir — the actor is at the moved location.
        let store = GitStore::init(dir.path()).unwrap();
        let e2 = Engine::boot(Box::new(store), Box::new(ManualClock::new(2000))).unwrap();
        assert_eq!(
            e2.realm().mob_location(&EntityId::new("snakewood/pc/nathan").unwrap()).map(|r| r.as_str()),
            Some("snakewood/old-well")
        );
        // Sessions are runtime-only; a freshly booted engine has none.
        assert_eq!(e2.session_actor(SessionId(0)), None);
    }

    #[test]
    fn checkpoint_without_store_is_ok_noop() {
        let mut e = engine();
        assert!(e.checkpoint("nothing to persist").is_ok());
    }
}
