use std::fs;
use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Repository, Signature, Time};
use walkdir::WalkDir;

use crate::store::{CommitId, StoreError, WorldStore};
use crate::{room_from_ron, room_to_ron, Room, World};

fn io_err<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Io(e.to_string())
}

fn git_err(e: git2::Error) -> StoreError {
    StoreError::Git(e.to_string())
}

/// A git-backed, version-controlled world store. One RON file per room under
/// `root/world/<zone>/rooms/<name>.ron`.
pub struct GitStore {
    root: PathBuf,
    repo: Repository,
}

impl GitStore {
    pub fn init(root: impl AsRef<Path>) -> Result<GitStore, StoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(io_err)?;
        let repo = match Repository::open(&root) {
            Ok(repo) => repo,
            Err(_) => Repository::init(&root).map_err(git_err)?,
        };
        Ok(GitStore { root, repo })
    }

    fn room_path(&self, room: &Room) -> PathBuf {
        self.root
            .join("world")
            .join(room.id.zone())
            .join("rooms")
            .join(format!("{}.ron", room.id.name()))
    }
}

impl WorldStore for GitStore {
    fn save_room(&mut self, room: &Room) -> Result<(), StoreError> {
        let path = self.room_path(room);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        fs::write(&path, room_to_ron(room)).map_err(io_err)?;
        Ok(())
    }

    fn load_all(&self) -> Result<World, StoreError> {
        let mut world = World::default();
        let world_dir = self.root.join("world");
        if !world_dir.exists() {
            return Ok(world);
        }
        for entry in WalkDir::new(&world_dir).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("ron") {
                continue;
            }
            let text = fs::read_to_string(path).map_err(io_err)?;
            let room = room_from_ron(&text).map_err(|e| StoreError::Parse(e.to_string()))?;
            world.insert_room(room);
        }
        Ok(world)
    }

    fn commit(&mut self, message: &str, epoch_seconds: i64) -> Result<CommitId, StoreError> {
        let mut index = self.repo.index().map_err(git_err)?;
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .map_err(git_err)?;
        index.write().map_err(git_err)?;
        let tree_oid = index.write_tree().map_err(git_err)?;
        let tree = self.repo.find_tree(tree_oid).map_err(git_err)?;

        let time = Time::new(epoch_seconds, 0);
        let sig = Signature::new("Snakewood", "world@snakewood.local", &time).map_err(git_err)?;

        let parent = match self.repo.head() {
            Ok(head) => Some(head.peel_to_commit().map_err(git_err)?),
            Err(_) => None,
        };
        let parents: Vec<&git2::Commit> = parent.iter().collect();

        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .map_err(git_err)?;
        Ok(CommitId(oid.to_string()))
    }

    fn commit_log(&self) -> Vec<String> {
        let mut messages = Vec::new();
        if let Ok(mut revwalk) = self.repo.revwalk() {
            if revwalk.push_head().is_ok() {
                for oid in revwalk.flatten() {
                    if let Ok(commit) = self.repo.find_commit(oid) {
                        messages.push(commit.message().unwrap_or("").to_string());
                    }
                }
            }
        }
        messages.reverse(); // oldest first
        messages
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use super::*;
    use crate::{Direction, EntityId};

    fn clearing() -> Room {
        let mut exits = BTreeMap::new();
        exits.insert(Direction::North, EntityId::new("snakewood/old-well").unwrap());
        Room {
            id: EntityId::new("snakewood/clearing").unwrap(),
            name: "Snakewood Clearing".to_string(),
            description: "A clearing.".to_string(),
            exits,
        }
    }

    fn old_well() -> Room {
        Room {
            id: EntityId::new("snakewood/old-well").unwrap(),
            name: "The Old Well".to_string(),
            description: "A crumbling stone well.".to_string(),
            exits: BTreeMap::new(),
        }
    }

    #[test]
    fn round_trips_through_git_and_reload() {
        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();
        store.save_room(&clearing()).unwrap();
        store.save_room(&old_well()).unwrap();
        store.commit("dig snakewood clearing and old well", 1_700_000_000).unwrap();

        // Fresh store over the same directory reloads an identical world.
        let reloaded = GitStore::init(dir.path()).unwrap().load_all().unwrap();
        let mut expected = World::default();
        expected.insert_room(clearing());
        expected.insert_room(old_well());
        assert_eq!(reloaded, expected);
    }

    #[test]
    fn commit_is_recorded_in_git_history() {
        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();
        store.save_room(&clearing()).unwrap();
        store.commit("dig snakewood clearing", 1_700_000_000).unwrap();
        assert_eq!(store.commit_log(), vec!["dig snakewood clearing".to_string()]);
    }

    #[test]
    fn writes_one_ron_file_per_room_at_expected_path() {
        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();
        store.save_room(&clearing()).unwrap();
        let expected = dir.path().join("world/snakewood/rooms/clearing.ron");
        assert!(expected.exists(), "expected room file at {expected:?}");
    }
}
