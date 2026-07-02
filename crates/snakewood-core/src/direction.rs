use serde::{Deserialize, Serialize};

/// A compass/vertical direction. Ordered so it can key a sorted map deterministically.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    North,
    South,
    East,
    West,
    Up,
    Down,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directions_sort_by_declaration_order() {
        let mut v = vec![Direction::Down, Direction::North, Direction::East];
        v.sort();
        assert_eq!(v, vec![Direction::North, Direction::East, Direction::Down]);
    }
}
