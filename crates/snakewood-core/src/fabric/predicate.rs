use serde::{Deserialize, Serialize};

use crate::{EntityId, Flag, Realm};

/// Which participant a predicate/effect refers to. `SelfMob` is the mob that
/// owns the responder (None for global rules).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum Party {
    Actor,
    SelfMob,
}

/// A guard, drawn from the fixed predicate vocabulary.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Predicate {
    Alive(Party),
    Conscious(Party),
}

fn resolve<'a>(party: Party, self_id: Option<&'a EntityId>, actor: &'a EntityId) -> Option<&'a EntityId> {
    match party {
        Party::Actor => Some(actor),
        Party::SelfMob => self_id,
    }
}

fn mob_has(realm: &Realm, who: Option<&EntityId>, flag: Flag) -> bool {
    match who.and_then(|id| realm.mob(id)) {
        Some(mob) => mob.has_flag(flag),
        None => false,
    }
}

/// Evaluate a predicate against current state. Unresolvable parties → false.
pub fn eval(realm: &Realm, pred: &Predicate, self_id: Option<&EntityId>, actor: &EntityId) -> bool {
    match pred {
        Predicate::Alive(p) => mob_has(realm, resolve(*p, self_id, actor), Flag::Alive),
        Predicate::Conscious(p) => mob_has(realm, resolve(*p, self_id, actor), Flag::Conscious),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{Mob, World};

    fn realm_with_goblin(alive: bool, conscious: bool) -> (Realm, EntityId, EntityId) {
        let mut flags = BTreeSet::new();
        if alive {
            flags.insert(Flag::Alive);
        }
        if conscious {
            flags.insert(Flag::Conscious);
        }
        let goblin_id = EntityId::new("snakewood/mob/goblin#1").unwrap();
        let actor_id = EntityId::new("snakewood/pc/nathan").unwrap();
        let mut realm = Realm::new(World::default());
        realm.insert_mob(Mob {
            id: goblin_id.clone(),
            name: "goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        });
        (realm, goblin_id, actor_id)
    }

    #[test]
    fn self_alive_true_when_alive() {
        let (realm, goblin, actor) = realm_with_goblin(true, true);
        assert!(eval(&realm, &Predicate::Alive(Party::SelfMob), Some(&goblin), &actor));
    }

    #[test]
    fn self_alive_false_when_dead() {
        let (realm, goblin, actor) = realm_with_goblin(false, true);
        assert!(!eval(&realm, &Predicate::Alive(Party::SelfMob), Some(&goblin), &actor));
    }

    #[test]
    fn self_predicate_false_when_no_self_id() {
        let (realm, _goblin, actor) = realm_with_goblin(true, true);
        assert!(!eval(&realm, &Predicate::Alive(Party::SelfMob), None, &actor));
    }

    #[test]
    fn conscious_reflects_flag() {
        let (realm, goblin, actor) = realm_with_goblin(true, false);
        assert!(!eval(&realm, &Predicate::Conscious(Party::SelfMob), Some(&goblin), &actor));
    }
}
