use crate::{Direction, EntityId};

/// A proposed, vetoable action entering the fabric.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Move {
        actor: EntityId,
        direction: Direction,
    },
    Look {
        actor: EntityId,
    },
}

/// A committed, factual, observable occurrence produced by the Commit phase.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Moved {
        actor: EntityId,
        from: EntityId,
        to: EntityId,
    },
    Looked {
        actor: EntityId,
        room: EntityId,
    },
}

impl Intent {
    /// The entity performing the intent.
    pub fn actor(&self) -> &EntityId {
        match self {
            Intent::Move { actor, .. } => actor,
            Intent::Look { actor } => actor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_accessor_returns_the_actor() {
        let a = EntityId::new("snakewood/pc/nathan").unwrap();
        let intent = Intent::Move {
            actor: a.clone(),
            direction: Direction::North,
        };
        assert_eq!(intent.actor(), &a);
        let look = Intent::Look { actor: a.clone() };
        assert_eq!(look.actor(), &a);
    }
}
