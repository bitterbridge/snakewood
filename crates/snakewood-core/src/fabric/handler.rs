use serde::{Deserialize, Serialize};

use crate::fabric::{Party, Predicate, Trigger};
use crate::EntityId;

/// An effect a handler runs when it is the salient responder.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Effect {
    Narrate(Party, String),
}

/// A handler's vote/result. `Traverse` allows movement to a room; `Block` denies;
/// `Allow` is a generic non-movement allow. (Redirect is deferred to a later stage.)
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Outcome {
    Traverse(EntityId),
    Block,
    Allow,
}

/// Salience band. Lower rank = more salient (narrates first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Participant,
    Structure,
    Global,
}

impl Band {
    pub fn rank(&self) -> u8 {
        match self {
            Band::Participant => 0,
            Band::Structure => 1,
            Band::Global => 2,
        }
    }
}

/// An entity-attached handler (lives on a `Mob`, Participant band).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Responder {
    pub on: Trigger,
    #[serde(default)]
    pub require: Vec<Predicate>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    pub outcome: Outcome,
    #[serde(default)]
    pub priority: i32,
}

/// A global, unattached handler (Global band).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Rule {
    pub on: Trigger,
    #[serde(default)]
    pub require: Vec<Predicate>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    pub outcome: Outcome,
    #[serde(default)]
    pub priority: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_rank_orders_participant_most_salient() {
        assert!(Band::Participant.rank() < Band::Structure.rank());
        assert!(Band::Structure.rank() < Band::Global.rank());
    }
}
