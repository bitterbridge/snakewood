//! snakewood-core: the pure, deterministic world model and storage layer.

pub mod direction;
pub mod id;
pub mod world;

pub use direction::Direction;
pub use id::{EntityId, IdError};
pub use world::{Room, World};

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds() {
        assert_eq!(2 + 2, 4);
    }
}
