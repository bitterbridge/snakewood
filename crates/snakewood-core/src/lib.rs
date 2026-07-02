//! snakewood-core: the pure, deterministic world model and storage layer.

pub mod direction;
pub mod fabric;
pub mod id;
pub mod mob;
pub mod realm;
pub mod serialize;
pub mod store;
pub mod world;

pub use direction::Direction;
pub use fabric::{Event, Intent};
pub use id::{EntityId, IdError};
pub use mob::{Flag, Mob};
pub use realm::Realm;
pub use serialize::{room_from_ron, room_to_ron};
pub use store::{CommitId, GitStore, MemoryStore, StoreError, WorldStore};
pub use world::{Room, World};

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds() {
        assert_eq!(2 + 2, 4);
    }
}
