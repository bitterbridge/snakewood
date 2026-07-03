use std::collections::BTreeMap;

use snakewood_core::{
    coalesce, dispatch, Admission, Direction, EntityId, Intent, IntentClass, Operator,
    PresentationKind, PresentationNode, RateLimiterState, Realm, Room, StoreError, WorldStore,
};

use crate::session::{Session, SessionId};
use crate::Clock;

/// Why a `dig` failed.
#[derive(Debug)]
pub enum DigError {
    NoSession,
    NoLocation,
    InvalidId(String),
    RoomExists,
    Store(StoreError),
}

/// The synchronous core of the daemon: owns the world, the clock, and sessions.
pub struct Engine {
    realm: Realm,
    clock: Box<dyn Clock>,
    sessions: BTreeMap<SessionId, Session>,
    next_session: u64,
    next_anon: u64,
    tick: u64,
    store: Option<Box<dyn WorldStore>>,
    snapshot_interval: Option<i64>,
    last_snapshot: i64,
    intent_queue: Vec<(SessionId, Intent)>,
    rate_limiter: RateLimiterState,
    drain_count: u64,
}

impl Engine {
    pub fn new(realm: Realm, clock: Box<dyn Clock>) -> Engine {
        Engine {
            realm,
            clock,
            sessions: BTreeMap::new(),
            next_session: 0,
            next_anon: 0,
            tick: 0,
            store: None,
            snapshot_interval: None,
            last_snapshot: 0,
            intent_queue: Vec::new(),
            rate_limiter: RateLimiterState::default(),
            drain_count: 0,
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

    /// Mint the next globally-unique anonymous-player sequence number.
    ///
    /// Shared across all transports (telnet, structured API) so two
    /// concurrently-connecting anonymous players never collide on the same
    /// `player/anon-N` id.
    pub fn mint_anon_seq(&mut self) -> u64 {
        let n = self.next_anon;
        self.next_anon += 1;
        n
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

    /// Authorize and buffer an intent for the next tick's drain. Like `submit`,
    /// a session may only enqueue intents for the actor it is bound to.
    pub fn enqueue(&mut self, id: SessionId, intent: Intent) {
        let authorized = matches!(self.sessions.get(&id), Some(s) if &s.actor == intent.actor());
        if !authorized {
            return;
        }
        self.intent_queue.push((id, intent));
    }

    /// How many times the intent queue has been drained (one per `tick`).
    pub fn drain_count(&self) -> u64 {
        self.drain_count
    }

    /// Sessions whose outbox currently holds undelivered presentation.
    pub fn sessions_with_pending(&self) -> Vec<SessionId> {
        self.sessions
            .iter()
            .filter(|(_, s)| !s.outbox.is_empty())
            .map(|(id, _)| *id)
            .collect()
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
        self.drain();
        self.tick
    }

    /// Drain the intent queue for the current tick: gate each intent through
    /// RateLimit operators, dispatch the admitted ones, coalesce the resulting
    /// directed presentation per recipient, and flush to session outboxes.
    fn drain(&mut self) {
        let queue = std::mem::take(&mut self.intent_queue);
        let tick = self.tick;
        let mut batched: Vec<(EntityId, PresentationNode)> = Vec::new();

        for (_sid, intent) in queue {
            let actor = intent.actor().clone();
            let class = IntentClass::of(&intent);
            match self
                .rate_limiter
                .admit(&self.realm.operators, class, &actor, tick)
            {
                Admission::Admit => {
                    let result = dispatch(&mut self.realm, intent);
                    batched.extend(result.messages);
                    // Notify broadcast to bystanders is deferred to M3; events
                    // stay in `result.events` unused here.
                }
                Admission::Drop { deny } => {
                    let text = deny.unwrap_or_else(|| self.realm.rate_limit_message.clone());
                    batched.push((actor, PresentationNode::Denied(text)));
                }
            }
        }

        // Kinds any Coalesce operator targets (union across all Coalesce ops).
        let coalesced_kinds: Vec<PresentationKind> = self
            .realm
            .operators
            .iter()
            .filter_map(|op| match op {
                Operator::Coalesce { on, .. } => Some(on.clone()),
                _ => None,
            })
            .flatten()
            .collect();

        // Distinct recipients in first-seen order (deterministic).
        let mut recipients: Vec<EntityId> = Vec::new();
        for (r, _) in &batched {
            if !recipients.contains(r) {
                recipients.push(r.clone());
            }
        }

        for recipient in recipients {
            let nodes: Vec<PresentationNode> = batched
                .iter()
                .filter(|(r, _)| *r == recipient)
                .map(|(_, n)| n.clone())
                .collect();
            let nodes = if coalesced_kinds.is_empty() {
                nodes
            } else {
                coalesce(nodes, &coalesced_kinds)
            };
            for session in self.sessions.values_mut() {
                if session.actor == recipient {
                    session.outbox.extend(nodes.iter().cloned());
                }
            }
        }

        self.drain_count += 1;
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

    /// Configure the on-interval snapshot cadence (seconds of injected time).
    pub fn set_snapshot_interval(&mut self, secs: i64) {
        self.snapshot_interval = Some(secs);
    }

    /// Commit an interval snapshot if the configured interval has elapsed since
    /// the last one. Returns whether it committed. Driven from the tick loop.
    pub fn maybe_snapshot(&mut self) -> Result<bool, StoreError> {
        let now = self.clock.now_unix();
        let due = matches!(self.snapshot_interval, Some(iv) if now - self.last_snapshot >= iv);
        if !due {
            return Ok(false);
        }
        let mut committed = false;
        if let Some(store) = self.store.as_mut() {
            store.save_realm(&self.realm)?;
            store.commit("interval snapshot", now)?;
            committed = true;
        }
        self.last_snapshot = now;
        Ok(committed)
    }

    /// OOC world-building: create a new room reached by `direction` from the
    /// session's current room, linked both ways, and checkpoint it.
    pub fn dig(
        &mut self,
        session: SessionId,
        direction: Direction,
        new_id: &str,
        name: &str,
        description: &str,
    ) -> Result<EntityId, DigError> {
        let actor = self
            .session_actor(session)
            .ok_or(DigError::NoSession)?
            .clone();
        let current = self
            .realm()
            .mob_location(&actor)
            .ok_or(DigError::NoLocation)?
            .clone();
        let new_room_id =
            EntityId::new(new_id).map_err(|_| DigError::InvalidId(new_id.to_string()))?;
        if self.realm().world.room(&new_room_id).is_some() {
            return Err(DigError::RoomExists);
        }
        // Create the new room with a back-exit to the current room.
        let mut exits = BTreeMap::new();
        exits.insert(direction.opposite(), current.clone());
        self.realm_mut().world.insert_room(Room {
            id: new_room_id.clone(),
            name: name.to_string(),
            description: description.to_string(),
            exits,
        });
        // Link the current room's exit to the new room.
        if let Some(room) = self.realm_mut().world.rooms.get_mut(&current) {
            room.exits.insert(direction, new_room_id.clone());
        }
        self.checkpoint(&format!("dig {new_id}"))
            .map_err(DigError::Store)?;
        Ok(new_room_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualClock;
    use snakewood_core::{
        Direction, Flag, GitStore, Intent, IntentClass, Mob, Operator, PresentationKind,
        PresentationNode, Room, Scope, World,
    };
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

    struct ArcClock(std::sync::Arc<ManualClock>);
    impl crate::Clock for ArcClock {
        fn now_unix(&self) -> i64 {
            self.0.now_unix()
        }
    }

    fn engine_with_store_and_actor(dir: &std::path::Path, start: i64) -> (Engine, SessionId) {
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
        let store = GitStore::init(dir).unwrap();
        let mut e = Engine::new(realm, Box::new(ManualClock::new(start)));
        e.attach_store(Box::new(store));
        let sid = e.connect(EntityId::new("snakewood/pc/nathan").unwrap());
        (e, sid)
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

    fn engine_with_actor_and_ops(ops: Vec<Operator>) -> (Engine, SessionId, EntityId) {
        let (mut e, sid, actor) = engine_with_actor();
        e.realm_mut().operators = ops;
        (e, sid, actor)
    }

    #[test]
    fn drain_rate_limits_moves_across_ticks() {
        // north then south is a round trip between the two rooms.
        let ops = vec![Operator::RateLimit {
            on: IntentClass::Move,
            per_ticks: 2,
            scope: Scope::PerActor,
            deny: Some("Too fast.".to_string()),
        }];
        let (mut e, sid, actor) = engine_with_actor_and_ops(ops);

        // Tick 1: enqueue two moves (N then S). Only the first is admitted.
        e.enqueue(
            sid,
            Intent::Move {
                actor: actor.clone(),
                direction: Direction::North,
            },
        );
        e.enqueue(
            sid,
            Intent::Move {
                actor: actor.clone(),
                direction: Direction::South,
            },
        );
        e.tick();
        // First move (north) committed; second dropped -> still at old-well.
        assert_eq!(
            e.realm().mob_location(&actor).map(|r| r.as_str()),
            Some("snakewood/old-well")
        );
        let out = e.poll(sid);
        // The dropped move produced a Denied node with the configured text.
        assert!(out
            .iter()
            .any(|n| *n == PresentationNode::Denied("Too fast.".to_string())));
    }

    #[test]
    fn drain_coalesces_repeated_room_views_in_one_tick() {
        // No rate limit; coalesce room-view kinds. Two Looks in one tick each emit a
        // full room view; they collapse to one. (Uses Look, not Move, so it works
        // with the one-way two-room test world.)
        let ops = vec![Operator::Coalesce {
            on: vec![
                PresentationKind::RoomName,
                PresentationKind::RoomDescription,
                PresentationKind::Exits,
                PresentationKind::Occupants,
            ],
            within_ticks: 1,
            scope: Scope::PerActor,
        }];
        let (mut e, sid, actor) = engine_with_actor_and_ops(ops);
        e.enqueue(
            sid,
            Intent::Look {
                actor: actor.clone(),
            },
        );
        e.enqueue(
            sid,
            Intent::Look {
                actor: actor.clone(),
            },
        );
        e.tick();
        let out = e.poll(sid);
        // Exactly one RoomName survives, naming the actor's room.
        let room_names: Vec<&PresentationNode> = out
            .iter()
            .filter(|n| matches!(n, PresentationNode::RoomName(_)))
            .collect();
        assert_eq!(room_names.len(), 1, "views not coalesced: {out:?}");
        assert_eq!(
            room_names[0],
            &PresentationNode::RoomName("Snakewood Clearing".to_string())
        );
    }

    #[test]
    fn drain_with_no_operators_dispatches_normally() {
        let (mut e, sid, actor) = engine_with_actor();
        e.enqueue(
            sid,
            Intent::Move {
                actor: actor.clone(),
                direction: Direction::North,
            },
        );
        e.tick();
        assert_eq!(
            e.realm().mob_location(&actor).map(|r| r.as_str()),
            Some("snakewood/old-well")
        );
        assert!(e
            .poll(sid)
            .iter()
            .any(|n| *n == PresentationNode::RoomName("The Old Well".to_string())));
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
    fn enqueue_authorizes_and_buffers_without_dispatching() {
        let (mut e, sid, actor) = engine_with_actor();
        // Enqueue does not dispatch: position unchanged, outbox empty, queue holds 1.
        e.enqueue(
            sid,
            Intent::Move {
                actor: actor.clone(),
                direction: Direction::North,
            },
        );
        assert_eq!(
            e.realm().mob_location(&actor).map(|r| r.as_str()),
            Some("snakewood/clearing")
        );
        assert!(e.poll(sid).is_empty());
        assert_eq!(e.drain_count(), 0);
    }

    #[test]
    fn enqueue_rejects_foreign_actor() {
        let (mut e, sid, _actor) = engine_with_actor();
        let stranger = EntityId::new("snakewood/pc/stranger").unwrap();
        e.enqueue(
            sid,
            Intent::Move {
                actor: stranger,
                direction: Direction::North,
            },
        );
        // Nothing queued: a later tick drains nothing and drain_count still advances by 1.
        let before = e.drain_count();
        e.tick();
        assert_eq!(e.drain_count(), before + 1);
        // The unauthorized move never happened.
        assert!(e.sessions_with_pending().is_empty());
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
            e.submit(
                sid,
                Intent::Move {
                    actor: EntityId::new("snakewood/pc/nathan").unwrap(),
                    direction: Direction::North,
                },
            );
            e.checkpoint("player moved north").unwrap();
        }
        // Second engine: boot from the same dir — the actor is at the moved location.
        let store = GitStore::init(dir.path()).unwrap();
        let e2 = Engine::boot(Box::new(store), Box::new(ManualClock::new(2000))).unwrap();
        assert_eq!(
            e2.realm()
                .mob_location(&EntityId::new("snakewood/pc/nathan").unwrap())
                .map(|r| r.as_str()),
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

    #[test]
    fn maybe_snapshot_waits_for_the_interval() {
        let dir = tempdir().unwrap();
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
        // Use a ManualClock we can advance; move it into the engine but keep a raw
        // control path by advancing before/after via a shared approach:
        let control = std::sync::Arc::new(ManualClock::new(0));
        let mut e = Engine::new(realm, Box::new(ArcClock(control.clone())));
        let dir_store = GitStore::init(dir.path()).unwrap();
        e.attach_store(Box::new(dir_store));
        e.set_snapshot_interval(3600);

        // Not enough time elapsed -> no snapshot.
        control.advance(100);
        assert!(!e.maybe_snapshot().unwrap());

        // Cross the interval -> snapshot commits.
        control.advance(3600);
        assert!(e.maybe_snapshot().unwrap());

        // Immediately after, not due again.
        assert!(!e.maybe_snapshot().unwrap());
    }

    #[test]
    fn maybe_snapshot_without_interval_is_false() {
        let dir = tempdir().unwrap();
        let (mut e, _sid) = engine_with_store_and_actor(dir.path(), 0);
        // No interval configured.
        assert!(!e.maybe_snapshot().unwrap());
    }

    #[test]
    fn dig_creates_linked_room_and_persists() {
        use snakewood_core::GitStore;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let new_room_str = "snakewood/hollow";
        {
            let mut realm = Realm::new(world_two_rooms()); // clearing --north--> old-well
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
            // Dig east from the clearing into a new hollow.
            let created = e
                .dig(
                    sid,
                    Direction::East,
                    new_room_str,
                    "A Hollow",
                    "A mossy hollow.",
                )
                .unwrap();
            assert_eq!(created.as_str(), new_room_str);
            // The new room exists with a back-exit west to the clearing.
            let hollow = e
                .realm()
                .world
                .room(&EntityId::new(new_room_str).unwrap())
                .unwrap();
            assert_eq!(
                hollow.exits.get(&Direction::West).map(|r| r.as_str()),
                Some("snakewood/clearing")
            );
            // The clearing now has an east exit to the hollow.
            let clearing = e
                .realm()
                .world
                .room(&EntityId::new("snakewood/clearing").unwrap())
                .unwrap();
            assert_eq!(
                clearing.exits.get(&Direction::East).map(|r| r.as_str()),
                Some(new_room_str)
            );
        }
        // Persisted: a fresh boot from the same dir has the dug room.
        let store = GitStore::init(dir.path()).unwrap();
        let e2 = Engine::boot(Box::new(store), Box::new(ManualClock::new(2000))).unwrap();
        assert!(e2
            .realm()
            .world
            .room(&EntityId::new(new_room_str).unwrap())
            .is_some());
    }

    #[test]
    fn dig_rejects_existing_room() {
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
        let mut e = Engine::new(realm, Box::new(ManualClock::new(0)));
        let sid = e.connect(EntityId::new("snakewood/pc/nathan").unwrap());
        // old-well already exists -> RoomExists.
        let result = e.dig(sid, Direction::East, "snakewood/old-well", "dup", "dup");
        assert!(matches!(result, Err(DigError::RoomExists)));
    }
}
