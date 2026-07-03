use std::collections::BTreeSet;

use snakewood_core::{EntityId, Flag, Mob};

use crate::{Engine, SessionId};

/// Spawn an anonymous player mob at `start_room` and connect a session to it.
///
/// The anonymous id is minted from the engine's global counter, so it is
/// unique across every transport (telnet, structured API) sharing this
/// engine — no two sessions can ever collide on the same `player/anon-N` id.
pub fn spawn_player(engine: &mut Engine, start_room: &EntityId) -> (SessionId, EntityId) {
    let seq = engine.mint_anon_seq();
    let actor = EntityId::new(format!("player/anon-{seq}")).expect("player id is valid");
    let mut flags = BTreeSet::new();
    flags.insert(Flag::Alive);
    flags.insert(Flag::Conscious);
    engine.realm_mut().insert_mob(Mob {
        id: actor.clone(),
        name: format!("Player{seq}"),
        location: start_room.clone(),
        flags,
        responders: Vec::new(),
    });
    let sid = engine.connect(actor.clone());
    (sid, actor)
}

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

/// Disconnect a player's session and remove its mob from the world.
pub fn despawn_player(engine: &mut Engine, sid: SessionId, actor: &EntityId) {
    engine.disconnect(sid);
    engine.realm_mut().mobs.remove(actor);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualClock;
    use snakewood_core::{Realm, World};

    #[test]
    fn spawn_places_player_and_binds_session() {
        let mut engine = Engine::new(Realm::new(World::default()), Box::new(ManualClock::new(0)));
        let start = EntityId::new("snakewood/clearing").unwrap();
        let (sid, actor) = spawn_player(&mut engine, &start);
        assert_eq!(actor.as_str(), "player/anon-0");
        assert_eq!(engine.session_actor(sid), Some(&actor));
        assert_eq!(
            engine.realm().mob_location(&actor).map(|r| r.as_str()),
            Some("snakewood/clearing")
        );
    }

    #[test]
    fn despawn_removes_session_and_mob() {
        let mut engine = Engine::new(Realm::new(World::default()), Box::new(ManualClock::new(0)));
        let start = EntityId::new("snakewood/clearing").unwrap();
        let (sid, actor) = spawn_player(&mut engine, &start);
        despawn_player(&mut engine, sid, &actor);
        assert_eq!(engine.session_actor(sid), None);
        assert!(engine.realm().mob(&actor).is_none());
    }

    #[test]
    fn attach_named_creates_then_reuses() {
        let mut engine = Engine::new(Realm::new(World::default()), Box::new(ManualClock::new(0)));
        let start = EntityId::new("snakewood/clearing").unwrap();
        let builder = EntityId::new("player/mcp-builder").unwrap();
        let s1 = attach_named(&mut engine, &builder, &start);
        assert_eq!(engine.session_actor(s1), Some(&builder));
        assert_eq!(
            engine.realm().mob_location(&builder).map(|r| r.as_str()),
            Some("snakewood/clearing")
        );
        // A second attach reuses the SAME mob (not recreated) and binds a new session.
        let s2 = attach_named(
            &mut engine,
            &builder,
            &EntityId::new("snakewood/elsewhere").unwrap(),
        );
        assert_ne!(s1, s2);
        // location unchanged — the mob was reused, not moved/recreated.
        assert_eq!(
            engine.realm().mob_location(&builder).map(|r| r.as_str()),
            Some("snakewood/clearing")
        );
    }

    #[test]
    fn spawn_player_ids_are_globally_unique_across_calls() {
        // Guards the Fix A invariant: the anon-id counter lives on the engine,
        // so repeated calls (representing different transports/connections)
        // never mint the same id.
        let mut engine = Engine::new(Realm::new(World::default()), Box::new(ManualClock::new(0)));
        let start = EntityId::new("snakewood/clearing").unwrap();
        let (_sid1, actor1) = spawn_player(&mut engine, &start);
        let (_sid2, actor2) = spawn_player(&mut engine, &start);
        assert_eq!(actor1.as_str(), "player/anon-0");
        assert_eq!(actor2.as_str(), "player/anon-1");
        assert_ne!(actor1, actor2);
    }
}
