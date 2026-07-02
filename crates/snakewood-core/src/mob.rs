use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::EntityId;

/// A boolean state marker on a mob. Guards (predicates) test these.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Flag {
    Alive,
    Conscious,
}

/// A live, located creature (both player-characters and NPCs, in Stage 2).
/// `responders` (data handlers) are added in Task 6 once `Responder` exists.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Mob {
    pub id: EntityId,
    pub name: String,
    /// The id of the room this mob currently occupies.
    pub location: EntityId,
    #[serde(default)]
    pub flags: BTreeSet<Flag>,
}

impl Mob {
    pub fn has_flag(&self, flag: Flag) -> bool {
        self.flags.contains(&flag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goblin() -> Mob {
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        flags.insert(Flag::Conscious);
        Mob {
            id: EntityId::new("snakewood/mob/goblin-1").unwrap(),
            name: "a snakewood goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
        }
    }

    #[test]
    fn has_flag_reflects_membership() {
        let g = goblin();
        assert!(g.has_flag(Flag::Alive));
        assert!(g.has_flag(Flag::Conscious));
    }

    #[test]
    fn missing_flag_is_false() {
        let mut g = goblin();
        g.flags.remove(&Flag::Alive);
        assert!(!g.has_flag(Flag::Alive));
    }
}
