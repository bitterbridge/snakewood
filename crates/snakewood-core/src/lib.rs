//! snakewood-core: the pure, deterministic world model and storage layer.

pub mod direction;
pub mod id;
pub mod world;
pub mod serialize;
pub mod store;

pub use direction::Direction;
pub use id::{EntityId, IdError};
pub use world::{Room, World};
pub use serialize::{room_from_ron, room_to_ron};
pub use store::{CommitId, GitStore, MemoryStore, StoreError, WorldStore};

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds() {
        assert_eq!(2 + 2, 4);
    }
}
