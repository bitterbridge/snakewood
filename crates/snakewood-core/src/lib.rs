//! snakewood-core: the pure, deterministic world model and storage layer.

pub mod direction;
pub mod id;

pub use direction::Direction;
pub use id::{EntityId, IdError};

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds() {
        assert_eq!(2 + 2, 4);
    }
}
