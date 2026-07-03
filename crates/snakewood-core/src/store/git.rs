use std::fs;
use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Repository, Signature, Time};
use walkdir::WalkDir;

use crate::store::{CommitId, StoreError, WorldStore};
use crate::{
    from_ron, room_from_ron, room_to_ron, to_ron, EntityId, Mob, Operator, Room, Rule, World,
};

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

    fn mob_path(&self, mob: &Mob) -> PathBuf {
        self.root
            .join("state")
            .join(mob.id.zone())
            .join("mobs")
            .join(format!("{}.ron", mob.id.name()))
    }

    fn mob_path_for_id(&self, id: &EntityId) -> PathBuf {
        self.root
            .join("state")
            .join(id.zone())
            .join("mobs")
            .join(format!("{}.ron", id.name()))
    }

    fn rules_path(&self) -> PathBuf {
        self.root.join("world").join("rules.ron")
    }

    fn operators_path(&self) -> PathBuf {
        self.root.join("world").join("operators.ron")
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
            // Rooms live under world/<zone>/rooms/<name>.ron; other authored
            // data (e.g. world/rules.ron) also sits under world/ but is not a
            // room, so only descend into "rooms" directories here.
            let is_room_file = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                == Some("rooms");
            if !is_room_file {
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
        // add_all stages new + modified files but not deletions of tracked
        // files; update_all removes index entries whose working-tree file is
        // gone, so a remove_mob/remove_room followed by commit actually drops
        // the entity from the committed tree.
        index.update_all(["*"].iter(), None).map_err(git_err)?;
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

    fn save_mob(&mut self, mob: &Mob) -> Result<(), StoreError> {
        let path = self.mob_path(mob);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        fs::write(&path, to_ron(mob)).map_err(io_err)?;
        Ok(())
    }

    fn remove_mob(&mut self, id: &EntityId) -> Result<(), StoreError> {
        let path = self.mob_path_for_id(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(e)),
        }
    }

    fn load_mobs(&self) -> Result<Vec<Mob>, StoreError> {
        let mut mobs = Vec::new();
        let state_dir = self.root.join("state");
        if !state_dir.exists() {
            return Ok(mobs);
        }
        for entry in WalkDir::new(&state_dir).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("ron") {
                continue;
            }
            let text = fs::read_to_string(path).map_err(io_err)?;
            let mob: Mob = from_ron(&text).map_err(|e| StoreError::Parse(e.to_string()))?;
            mobs.push(mob);
        }
        // Deterministic order by id.
        mobs.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        Ok(mobs)
    }

    fn save_rules(&mut self, rules: &[Rule]) -> Result<(), StoreError> {
        let path = self.rules_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        fs::write(&path, to_ron(rules)).map_err(io_err)?;
        Ok(())
    }

    fn load_rules(&self) -> Result<Vec<Rule>, StoreError> {
        let path = self.rules_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&path).map_err(io_err)?;
        from_ron(&text).map_err(|e| StoreError::Parse(e.to_string()))
    }

    fn save_operators(&mut self, operators: &[Operator]) -> Result<(), StoreError> {
        let path = self.operators_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        fs::write(&path, to_ron(operators)).map_err(io_err)?;
        Ok(())
    }

    fn load_operators(&self) -> Result<Vec<Operator>, StoreError> {
        let path = self.operators_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&path).map_err(io_err)?;
        from_ron(&text).map_err(|e| StoreError::Parse(e.to_string()))
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
        exits.insert(
            Direction::North,
            EntityId::new("snakewood/old-well").unwrap(),
        );
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
        store
            .commit("dig snakewood clearing and old well", 1_700_000_000)
            .unwrap();

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
        store
            .commit("dig snakewood clearing", 1_700_000_000)
            .unwrap();
        assert_eq!(
            store.commit_log(),
            vec!["dig snakewood clearing".to_string()]
        );
    }

    #[test]
    fn writes_one_ron_file_per_room_at_expected_path() {
        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();
        store.save_room(&clearing()).unwrap();
        let expected = dir.path().join("world/snakewood/rooms/clearing.ron");
        assert!(expected.exists(), "expected room file at {expected:?}");
    }

    #[test]
    fn realm_round_trips_through_git() {
        use crate::fabric::{Outcome, Rule, Trigger};
        use crate::{Flag, Mob};
        use std::collections::BTreeSet;

        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();

        let mut realm = crate::Realm::new({
            let mut w = World::default();
            w.insert_room(clearing());
            w
        });
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        realm.insert_mob(Mob {
            id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
            name: "a goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        });
        realm.rules.push(Rule {
            on: Trigger::AnyMove,
            require: Vec::new(),
            effects: Vec::new(),
            outcome: Outcome::Allow,
            priority: 0,
        });

        store.save_realm(&realm).unwrap();
        store.commit("save realm", 1_700_000_000).unwrap();

        let reloaded = GitStore::init(dir.path()).unwrap().load_realm().unwrap();
        assert_eq!(reloaded.world, realm.world);
        assert_eq!(reloaded.mobs, realm.mobs);
        assert_eq!(reloaded.rules, realm.rules);
    }

    #[test]
    fn operators_round_trip_through_git() {
        use crate::{IntentClass, Operator, Scope};
        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();
        let mut realm = crate::Realm::new({
            let mut w = World::default();
            w.insert_room(clearing());
            w
        });
        realm.operators.push(Operator::RateLimit {
            on: IntentClass::Move,
            per_ticks: 4,
            scope: Scope::PerActor,
            deny: Some("Slow down.".to_string()),
        });
        store.save_realm(&realm).unwrap();
        store
            .commit("save realm with operators", 1_700_000_000)
            .unwrap();

        let reloaded = GitStore::init(dir.path()).unwrap().load_realm().unwrap();
        assert_eq!(reloaded.operators, realm.operators);
        // Written at the world/ root, parallel to rules.ron.
        assert!(dir.path().join("world/operators.ron").exists());
    }

    #[test]
    fn commit_stages_deletions() {
        use crate::{Flag, Mob};
        use std::collections::BTreeSet;

        let dir = tempdir().unwrap();
        let mut store = GitStore::init(dir.path()).unwrap();
        let mut flags = BTreeSet::new();
        flags.insert(Flag::Alive);
        let goblin = Mob {
            id: EntityId::new("snakewood/mob/goblin#1").unwrap(),
            name: "a goblin".to_string(),
            location: EntityId::new("snakewood/clearing").unwrap(),
            flags,
            responders: Vec::new(),
        };
        store.save_mob(&goblin).unwrap();
        store.commit("spawn goblin", 1_700_000_000).unwrap();

        // Remove and commit the deletion.
        store.remove_mob(&goblin.id).unwrap();
        store.commit("goblin dies", 1_700_000_100).unwrap();

        // A fresh CLONE of the committed repo must not contain the goblin —
        // proving the deletion was staged into the tree, not just the working dir.
        let clone_dir = tempdir().unwrap();
        let repo_url = dir.path().to_str().unwrap();
        git2::Repository::clone(repo_url, clone_dir.path()).unwrap();
        let reloaded = GitStore::init(clone_dir.path())
            .unwrap()
            .load_mobs()
            .unwrap();
        assert!(
            reloaded.is_empty(),
            "deleted mob must be gone from committed tree: {reloaded:?}"
        );
    }
}
