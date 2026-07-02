use std::collections::BTreeSet;

use snakewood_core::{EntityId, Flag, Mob};

use crate::{Engine, SessionId};

/// Spawn an anonymous player mob at `start_room` and connect a session to it.
pub fn spawn_player(engine: &mut Engine, start_room: &EntityId, seq: u64) -> (SessionId, EntityId) {
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
        let (sid, actor) = spawn_player(&mut engine, &start, 7);
        assert_eq!(actor.as_str(), "player/anon-7");
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
        let (sid, actor) = spawn_player(&mut engine, &start, 1);
        despawn_player(&mut engine, sid, &actor);
        assert_eq!(engine.session_actor(sid), None);
        assert!(engine.realm().mob(&actor).is_none());
    }
}
