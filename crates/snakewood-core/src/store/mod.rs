use crate::{Room, World};

pub mod memory;

pub use memory::MemoryStore;

#[derive(Debug)]
pub enum StoreError {
    Io(String),
    Parse(String),
    Git(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitId(pub String);

/// A place authored world data is persisted and versioned. Core logic depends
/// only on this trait; implementations own all filesystem/git contact.
pub trait WorldStore {
    /// Persist a single room (one file per entity, in git-backed impls).
    fn save_room(&mut self, room: &Room) -> Result<(), StoreError>;

    /// Load the entire world from storage.
    fn load_all(&self) -> Result<World, StoreError>;

    /// Commit all pending saves with `message`, timestamped at `epoch_seconds`.
    fn commit(&mut self, message: &str, epoch_seconds: i64) -> Result<CommitId, StoreError>;

    /// Commit messages recorded so far, oldest first.
    fn commit_log(&self) -> Vec<String>;
}
