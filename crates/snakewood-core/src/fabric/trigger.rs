use serde::{Deserialize, Serialize};

use crate::fabric::Intent;
use crate::Direction;

/// A pattern a handler listens for. `AnyMove` matches movement in any direction.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Trigger {
    Move(Direction),
    AnyMove,
    Look,
}

impl Trigger {
    pub fn matches(&self, intent: &Intent) -> bool {
        match (self, intent) {
            (Trigger::Move(d), Intent::Move { direction, .. }) => d == direction,
            (Trigger::AnyMove, Intent::Move { .. }) => true,
            (Trigger::Look, Intent::Look { .. }) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntityId;

    fn move_north() -> Intent {
        Intent::Move {
            actor: EntityId::new("snakewood/pc/nathan").unwrap(),
            direction: Direction::North,
        }
    }

    #[test]
    fn exact_direction_matches_only_that_direction() {
        assert!(Trigger::Move(Direction::North).matches(&move_north()));
        assert!(!Trigger::Move(Direction::South).matches(&move_north()));
    }

    #[test]
    fn any_move_matches_all_moves_but_not_look() {
        let look = Intent::Look { actor: EntityId::new("snakewood/pc/nathan").unwrap() };
        assert!(Trigger::AnyMove.matches(&move_north()));
        assert!(!Trigger::AnyMove.matches(&look));
    }

    #[test]
    fn look_trigger_matches_look() {
        let look = Intent::Look { actor: EntityId::new("snakewood/pc/nathan").unwrap() };
        assert!(Trigger::Look.matches(&look));
        assert!(!Trigger::Look.matches(&move_north()));
    }
}
