use crate::{EntityId, Mob, Realm, Room, Rule, World};

pub mod git;
pub mod memory;

pub use git::GitStore;
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

    /// Persist a single live mob instance (to `state/` in git-backed impls).
    fn save_mob(&mut self, mob: &Mob) -> Result<(), StoreError>;

    /// Remove a persisted mob by id (no-op if it isn't stored).
    fn remove_mob(&mut self, id: &EntityId) -> Result<(), StoreError>;

    /// Load all live mob instances.
    fn load_mobs(&self) -> Result<Vec<Mob>, StoreError>;

    /// Persist the global rule list (authored, `world/` in git-backed impls).
    fn save_rules(&mut self, rules: &[Rule]) -> Result<(), StoreError>;

    /// Load the global rule list (empty if none persisted).
    fn load_rules(&self) -> Result<Vec<Rule>, StoreError>;

    /// Load the entire live realm: authored rooms + live mobs + global rules.
    fn load_realm(&self) -> Result<Realm, StoreError> {
        let mut realm = Realm::new(self.load_all()?);
        for mob in self.load_mobs()? {
            realm.insert_mob(mob);
        }
        realm.rules = self.load_rules()?;
        Ok(realm)
    }

    /// Persist an entire realm: every room, every mob, and the rule list.
    fn save_realm(&mut self, realm: &Realm) -> Result<(), StoreError> {
        for room in realm.world.rooms.values() {
            self.save_room(room)?;
        }
        for mob in realm.mobs.values() {
            self.save_mob(mob)?;
        }
        self.save_rules(&realm.rules)?;
        Ok(())
    }
}
