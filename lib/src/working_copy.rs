// Copyright 2020 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::cell::{RefCell, RefMut};
use std::collections::{BTreeMap, HashSet};
use std::convert::TryInto;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::ops::Bound;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(windows)]
use std::os::windows::fs::symlink_file;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use protobuf::Message;
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::commit::Commit;
use crate::commit_builder::CommitBuilder;
use crate::gitignore::GitIgnoreFile;
use crate::lock::FileLock;
use crate::matchers::EverythingMatcher;
use crate::repo::ReadonlyRepo;
use crate::repo_path::{RepoPath, RepoPathComponent, RepoPathJoin};
use crate::settings::UserSettings;
use crate::store::{CommitId, FileId, MillisSinceEpoch, StoreError, SymlinkId, TreeId, TreeValue};
use crate::store_wrapper::StoreWrapper;
use crate::tree::Diff;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum FileType {
    Normal,
    Executable,
    Symlink,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FileState {
    pub file_type: FileType,
    pub mtime: MillisSinceEpoch,
    pub size: u64,
    /* TODO: What else do we need here? Git stores a lot of fields.
     * TODO: Could possibly handle case-insensitive file systems keeping an
     *       Option<PathBuf> with the actual path here. */
}

impl FileState {
    fn null() -> FileState {
        FileState {
            file_type: FileType::Normal,
            mtime: MillisSinceEpoch(0),
            size: 0,
        }
    }

    fn mark_executable(&mut self, executable: bool) {
        match (&self.file_type, executable) {
            (FileType::Normal, true) => self.file_type = FileType::Executable,
            (FileType::Executable, false) => self.file_type = FileType::Normal,
            _ => {}
        }
    }
}

pub struct TreeState {
    store: Arc<StoreWrapper>,
    working_copy_path: PathBuf,
    state_path: PathBuf,
    tree_id: TreeId,
    file_states: BTreeMap<RepoPath, FileState>,
    read_time: MillisSinceEpoch,
}

fn file_state_from_proto(proto: &crate::protos::working_copy::FileState) -> FileState {
    let file_type = match proto.file_type {
        crate::protos::working_copy::FileType::Normal => FileType::Normal,
        crate::protos::working_copy::FileType::Symlink => FileType::Symlink,
        crate::protos::working_copy::FileType::Executable => FileType::Executable,
    };
    FileState {
        file_type,
        mtime: MillisSinceEpoch(proto.mtime_millis_since_epoch),
        size: proto.size,
    }
}

fn file_state_to_proto(file_state: &FileState) -> crate::protos::working_copy::FileState {
    let mut proto = crate::protos::working_copy::FileState::new();
    let file_type = match &file_state.file_type {
        FileType::Normal => crate::protos::working_copy::FileType::Normal,
        FileType::Symlink => crate::protos::working_copy::FileType::Symlink,
        FileType::Executable => crate::protos::working_copy::FileType::Executable,
    };
    proto.file_type = file_type;
    proto.mtime_millis_since_epoch = file_state.mtime.0;
    proto.size = file_state.size;
    proto
}

fn file_states_from_proto(
    proto: &crate::protos::working_copy::TreeState,
) -> BTreeMap<RepoPath, FileState> {
    let mut file_states = BTreeMap::new();
    for (path_str, proto_file_state) in &proto.file_states {
        let path = RepoPath::from_internal_string(path_str.as_str());
        file_states.insert(path, file_state_from_proto(&proto_file_state));
    }
    file_states
}

fn create_parent_dirs(disk_path: &Path) {
    fs::create_dir_all(disk_path.parent().unwrap())
        .unwrap_or_else(|_| panic!("failed to create parent directories for {:?}", &disk_path));
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CheckoutStats {
    pub updated_files: u32,
    pub added_files: u32,
    pub removed_files: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CheckoutError {
    #[error("Update target not found")]
    TargetNotFound,
    // The current checkout was deleted, maybe by an overly aggressive GC that happened while
    // the current process was running.
    #[error("Current checkout not found")]
    SourceNotFound,
    // Another process checked out a commit while the current process was running (after the
    // working copy was read by the current process).
    #[error("Concurrent checkout")]
    ConcurrentCheckout,
    #[error("Internal error: {0:?}")]
    InternalStoreError(StoreError),
}

impl TreeState {
    pub fn current_tree_id(&self) -> &TreeId {
        &self.tree_id
    }

    pub fn file_states(&self) -> &BTreeMap<RepoPath, FileState> {
        &self.file_states
    }

    pub fn init(
        store: Arc<StoreWrapper>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
    ) -> TreeState {
        let mut wc = TreeState::empty(store, working_copy_path, state_path);
        wc.save();
        wc
    }

    fn empty(
        store: Arc<StoreWrapper>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
    ) -> TreeState {
        let tree_id = store.empty_tree_id().clone();
        // Canonicalize the working copy path because "repo/." makes libgit2 think that
        // everything should be ignored
        TreeState {
            store,
            working_copy_path: working_copy_path.canonicalize().unwrap(),
            state_path,
            tree_id,
            file_states: BTreeMap::new(),
            read_time: MillisSinceEpoch(0),
        }
    }

    pub fn load(
        store: Arc<StoreWrapper>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
    ) -> TreeState {
        let maybe_file = File::open(state_path.join("tree_state"));
        let file = match maybe_file {
            Err(ref err) if err.kind() == std::io::ErrorKind::NotFound => {
                return TreeState::init(store, working_copy_path, state_path);
            }
            result => result.unwrap(),
        };

        let mut wc = TreeState::empty(store, working_copy_path, state_path);
        wc.read(file);
        wc
    }

    fn update_read_time(&mut self) {
        let own_file_state = self
            .file_state(&self.state_path.join("tree_state"))
            .unwrap_or_else(FileState::null);
        self.read_time = own_file_state.mtime;
    }

    fn read(&mut self, mut file: File) {
        self.update_read_time();
        let proto: crate::protos::working_copy::TreeState =
            Message::parse_from_reader(&mut file).unwrap();
        self.tree_id = TreeId(proto.tree_id.clone());
        self.file_states = file_states_from_proto(&proto);
    }

    fn save(&mut self) {
        let mut proto = crate::protos::working_copy::TreeState::new();
        proto.tree_id = self.tree_id.0.clone();
        for (file, file_state) in &self.file_states {
            proto.file_states.insert(
                file.to_internal_file_string(),
                file_state_to_proto(file_state),
            );
        }

        let mut temp_file = NamedTempFile::new_in(&self.state_path).unwrap();
        // update read time while we still have the file open for writes, so we know
        // there is no unknown data in it
        self.update_read_time();
        proto.write_to_writer(temp_file.as_file_mut()).unwrap();
        temp_file
            .persist(self.state_path.join("tree_state"))
            .unwrap();
    }

    fn file_state(&self, path: &Path) -> Option<FileState> {
        let metadata = path.symlink_metadata().ok()?;
        let time = metadata.modified().unwrap();
        let since_epoch = time.duration_since(UNIX_EPOCH).unwrap();
        let mtime = MillisSinceEpoch(since_epoch.as_millis().try_into().unwrap());
        let size = metadata.len();
        let metadata_file_type = metadata.file_type();
        let file_type = if metadata_file_type.is_dir() {
            panic!("expected file, not directory: {:?}", path);
        } else if metadata_file_type.is_symlink() {
            FileType::Symlink
        } else {
            #[cfg(unix)]
            let mode = metadata.permissions().mode();
            #[cfg(windows)]
            let mode = 0;
            if mode & 0o111 != 0 {
                FileType::Executable
            } else {
                FileType::Normal
            }
        };
        Some(FileState {
            file_type,
            mtime,
            size,
        })
    }

    fn write_file_to_store(&self, path: &RepoPath, disk_path: &Path) -> FileId {
        let file = File::open(disk_path).unwrap();
        self.store.write_file(path, &mut Box::new(file)).unwrap()
    }

    fn write_symlink_to_store(&self, path: &RepoPath, disk_path: &Path) -> SymlinkId {
        let target = disk_path.read_link().unwrap();
        let str_target = target.to_str().unwrap();
        self.store.write_symlink(path, str_target).unwrap()
    }

    fn try_chain_gitignore(
        base: &Arc<GitIgnoreFile>,
        prefix: &str,
        file: PathBuf,
    ) -> Arc<GitIgnoreFile> {
        if file.is_file() {
            let mut file = File::open(file).unwrap();
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).unwrap();
            if let Ok(chained) = base.chain(prefix, &buf) {
                chained
            } else {
                base.clone()
            }
        } else {
            base.clone()
        }
    }

    // Look for changes to the working copy. If there are any changes, create
    // a new tree from it and return it, and also update the dirstate on disk.
    // TODO: respect ignores
    pub fn write_tree(&mut self) -> &TreeId {
        // We create a temporary git repo with the working copy shared with ours only
        // so we can use libgit2's .gitignore check.
        // TODO: We should probably have the caller pass in the home directory to the
        // library crate instead of depending on $HOME directly here. We should also
        // have the caller (within the library crate) chain that the
        // .jj/git/info/exclude file if we're inside a git-backed repo.
        let mut git_ignore = GitIgnoreFile::empty();
        if let Ok(home_dir) = std::env::var("HOME") {
            let home_dir_path = PathBuf::from(home_dir);
            git_ignore =
                TreeState::try_chain_gitignore(&git_ignore, "", home_dir_path.join(".gitignore"));
        }

        let mut work = vec![(RepoPath::root(), self.working_copy_path.clone(), git_ignore)];
        let mut tree_builder = self.store.tree_builder(self.tree_id.clone());
        let mut deleted_files: HashSet<&RepoPath> = self.file_states.keys().collect();
        let mut modified_files = BTreeMap::new();
        while !work.is_empty() {
            let (dir, disk_dir, git_ignore) = work.pop().unwrap();
            let git_ignore = TreeState::try_chain_gitignore(
                &git_ignore,
                &dir.to_internal_dir_string(),
                disk_dir.join(".gitignore"),
            );
            for maybe_entry in disk_dir.read_dir().unwrap() {
                let entry = maybe_entry.unwrap();
                let file_type = entry.file_type().unwrap();
                let file_name = entry.file_name();
                let name = file_name.to_str().unwrap();
                if name == ".jj" {
                    continue;
                }
                let sub_path = dir.join(&RepoPathComponent::from(name));
                if file_type.is_dir() {
                    if git_ignore.matches_all_files_in(&sub_path.to_internal_dir_string()) {
                        // If the whole directory is ignored, skip it unless we're already tracking
                        // some file in it. TODO: This is pretty ugly... Also, we should
                        // optimize it to check exactly the already-tracked files (we know that
                        // we won't have to consider new files in the directory).
                        let first_file_in_dir = dir.join(&RepoPathComponent::from("\0"));
                        if let Some((maybe_subdir_file, _)) = self
                            .file_states
                            .range((Bound::Included(&first_file_in_dir), Bound::Unbounded))
                            .next()
                        {
                            if !dir.contains(&maybe_subdir_file.parent().unwrap()) {
                                continue;
                            }
                        }
                    }
                    let disk_subdir = disk_dir.join(file_name);
                    work.push((sub_path, disk_subdir, git_ignore.clone()));
                } else {
                    let disk_file = disk_dir.join(file_name);
                    deleted_files.remove(&sub_path);
                    let new_file_state = self.file_state(&entry.path()).unwrap();
                    let clean;
                    let executable;
                    match self.file_states.get(&sub_path) {
                        None => {
                            // untracked
                            if git_ignore.matches_file(&sub_path.to_internal_file_string()) {
                                continue;
                            }
                            clean = false;

                            executable = new_file_state.file_type == FileType::Executable;
                        }
                        Some(current_entry) => {
                            clean = current_entry == &new_file_state
                                && current_entry.mtime < self.read_time;
                            #[cfg(windows)]
                            {
                                // On Windows, we preserve the state we had recorded
                                // when we wrote the file.
                                executable = current_entry.file_type == FileType::Executable
                            }
                            #[cfg(unix)]
                            {
                                executable = new_file_state.file_type == FileType::Executable
                            }
                        }
                    };
                    if !clean {
                        let file_value = match new_file_state.file_type {
                            FileType::Normal | FileType::Executable => {
                                let id = self.write_file_to_store(&sub_path, &disk_file);
                                TreeValue::Normal { id, executable }
                            }
                            FileType::Symlink => {
                                let id = self.write_symlink_to_store(&sub_path, &disk_file);
                                TreeValue::Symlink(id)
                            }
                        };
                        tree_builder.set(sub_path.clone(), file_value);
                        modified_files.insert(sub_path, new_file_state);
                    }
                }
            }
        }

        let deleted_files: Vec<RepoPath> = deleted_files.iter().cloned().cloned().collect();

        for file in &deleted_files {
            self.file_states.remove(file);
            tree_builder.remove(file.clone());
        }
        for (file, file_state) in modified_files {
            self.file_states.insert(file, file_state);
        }
        self.tree_id = tree_builder.write_tree();
        self.save();
        &self.tree_id
    }

    fn write_file(
        &self,
        disk_path: &Path,
        path: &RepoPath,
        id: &FileId,
        executable: bool,
    ) -> FileState {
        create_parent_dirs(disk_path);
        // TODO: Check that we're not overwriting an un-ignored file here (which might
        // be created by a concurrent process).
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(disk_path)
            .unwrap_or_else(|err| panic!("failed to open {:?} for write: {:?}", &disk_path, err));
        let mut contents = self.store.read_file(path, id).unwrap();
        std::io::copy(&mut contents, &mut file).unwrap();
        self.set_executable(disk_path, executable);
        // Read the file state while we still have the file open. That way, know that
        // the file exists, and the stat information is most likely accurate,
        // except for other processes modifying the file concurrently (The mtime is set
        // at write time and won't change when we close the file.)
        let mut file_state = self.file_state(&disk_path).unwrap();
        // Make sure the state we record is what we tried to set above. This is mostly
        // for Windows, since the executable bit is not reflected in the file system
        // there.
        file_state.mark_executable(executable);
        file_state
    }

    fn write_symlink(&self, disk_path: &Path, path: &RepoPath, id: &SymlinkId) -> FileState {
        create_parent_dirs(disk_path);
        #[cfg(windows)]
        {
            println!("ignoring symlink at {:?}", path);
        }
        #[cfg(unix)]
        {
            let target = self.store.read_symlink(path, id).unwrap();
            let target = PathBuf::from(&target);
            symlink(target, disk_path).unwrap();
        }
        self.file_state(&disk_path).unwrap()
    }

    fn set_executable(&self, disk_path: &Path, executable: bool) {
        #[cfg(windows)]
        {
            return;
        }
        #[cfg(unix)]
        {
            let mode = if executable { 0o755 } else { 0o644 };
            fs::set_permissions(disk_path, fs::Permissions::from_mode(mode)).unwrap();
        }
    }

    pub fn check_out(&mut self, tree_id: TreeId) -> Result<CheckoutStats, CheckoutError> {
        let old_tree = self
            .store
            .get_tree(&RepoPath::root(), &self.tree_id)
            .map_err(|err| match err {
                StoreError::NotFound => CheckoutError::SourceNotFound,
                other => CheckoutError::InternalStoreError(other),
            })?;
        let new_tree =
            self.store
                .get_tree(&RepoPath::root(), &tree_id)
                .map_err(|err| match err {
                    StoreError::NotFound => CheckoutError::TargetNotFound,
                    other => CheckoutError::InternalStoreError(other),
                })?;

        let mut stats = CheckoutStats {
            updated_files: 0,
            added_files: 0,
            removed_files: 0,
        };

        for (path, diff) in old_tree.diff(&new_tree, &EverythingMatcher) {
            let disk_path = path.to_fs_path(&self.working_copy_path);

            // TODO: Check that the file has not changed before overwriting/removing it.
            match diff {
                Diff::Removed(_before) => {
                    fs::remove_file(&disk_path).ok();
                    let mut parent_dir = disk_path.parent().unwrap();
                    loop {
                        if fs::remove_dir(&parent_dir).is_err() {
                            break;
                        }
                        parent_dir = parent_dir.parent().unwrap();
                    }
                    self.file_states.remove(&path);
                    stats.removed_files += 1;
                }
                Diff::Added(after) => {
                    let file_state = match after {
                        TreeValue::Normal { id, executable } => {
                            self.write_file(&disk_path, &path, &id, executable)
                        }
                        TreeValue::Symlink(id) => self.write_symlink(&disk_path, &path, &id),
                        TreeValue::GitSubmodule(_id) => {
                            println!("ignoring git submodule at {:?}", path);
                            continue;
                        }
                        TreeValue::Tree(_id) => {
                            panic!("unexpected tree entry in diff at {:?}", path);
                        }
                        TreeValue::Conflict(_id) => {
                            panic!(
                                "conflicts cannot be represented in the working copy: {:?}",
                                path
                            );
                        }
                    };
                    self.file_states.insert(path.clone(), file_state);
                    stats.added_files += 1;
                }
                Diff::Modified(before, after) => {
                    fs::remove_file(&disk_path).ok();
                    let file_state = match (before, after) {
                        (
                            TreeValue::Normal {
                                id: old_id,
                                executable: old_executable,
                            },
                            TreeValue::Normal { id, executable },
                        ) if id == old_id => {
                            // Optimization for when only the executable bit changed
                            assert_ne!(executable, old_executable);
                            self.set_executable(&disk_path, executable);
                            let mut file_state = self.file_states.get(&path).unwrap().clone();
                            file_state.mark_executable(executable);
                            file_state
                        }
                        (_, TreeValue::Normal { id, executable }) => {
                            self.write_file(&disk_path, &path, &id, executable)
                        }
                        (_, TreeValue::Symlink(id)) => self.write_symlink(&disk_path, &path, &id),
                        (_, TreeValue::GitSubmodule(_id)) => {
                            println!("ignoring git submodule at {:?}", path);
                            self.file_states.remove(&path);
                            continue;
                        }
                        (_, TreeValue::Tree(_id)) => {
                            panic!("unexpected tree entry in diff at {:?}", path);
                        }
                        (_, TreeValue::Conflict(_id)) => {
                            panic!(
                                "conflicts cannot be represented in the working copy: {:?}",
                                path
                            );
                        }
                    };

                    self.file_states.insert(path.clone(), file_state);
                    stats.updated_files += 1;
                }
            }
        }
        self.tree_id = tree_id;
        self.save();
        Ok(stats)
    }
}

pub struct WorkingCopy {
    store: Arc<StoreWrapper>,
    working_copy_path: PathBuf,
    state_path: PathBuf,
    commit_id: RefCell<Option<CommitId>>,
    tree_state: RefCell<Option<TreeState>>,
    // cached commit
    commit: RefCell<Option<Commit>>,
}

impl WorkingCopy {
    pub fn init(
        store: Arc<StoreWrapper>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
    ) -> WorkingCopy {
        // Leave the commit_id empty so a subsequent call to check out the root revision
        // will have an effect.
        let proto = crate::protos::working_copy::Checkout::new();
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(state_path.join("checkout"))
            .unwrap();
        proto.write_to_writer(&mut file).unwrap();
        WorkingCopy {
            store,
            working_copy_path,
            state_path,
            commit_id: RefCell::new(None),
            tree_state: RefCell::new(None),
            commit: RefCell::new(None),
        }
    }

    pub fn load(
        store: Arc<StoreWrapper>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
    ) -> WorkingCopy {
        WorkingCopy {
            store,
            working_copy_path,
            state_path,
            commit_id: RefCell::new(None),
            tree_state: RefCell::new(None),
            commit: RefCell::new(None),
        }
    }

    fn write_proto(&self, proto: crate::protos::working_copy::Checkout) {
        let mut temp_file = NamedTempFile::new_in(&self.state_path).unwrap();
        proto.write_to_writer(temp_file.as_file_mut()).unwrap();
        temp_file.persist(self.state_path.join("checkout")).unwrap();
    }

    fn read_proto(&self) -> crate::protos::working_copy::Checkout {
        let mut file = File::open(self.state_path.join("checkout")).unwrap();
        Message::parse_from_reader(&mut file).unwrap()
    }

    /// The id of the commit that's currently checked out in the working copy.
    /// Note that the View is the source of truth for which commit *should*
    /// be checked out. That should be kept up to date within a Transaction.
    /// The WorkingCopy is only updated at the end.
    pub fn current_commit_id(&self) -> CommitId {
        if self.commit_id.borrow().is_none() {
            let proto = self.read_proto();
            let commit_id = CommitId(proto.commit_id);
            self.commit_id.replace(Some(commit_id));
        }

        self.commit_id.borrow().as_ref().unwrap().clone()
    }

    /// The commit that's currently checked out in the working copy. Note that
    /// the View is the source of truth for which commit *should* be checked
    /// out. That should be kept up to date within a Transaction. The
    /// WorkingCopy is only updated at the end.
    pub fn current_commit(&self) -> Commit {
        let commit_id = self.current_commit_id();
        let stale = match self.commit.borrow().as_ref() {
            None => true,
            Some(value) => value.id() != &commit_id,
        };
        if stale {
            self.commit
                .replace(Some(self.store.get_commit(&commit_id).unwrap()));
        }
        self.commit.borrow().as_ref().unwrap().clone()
    }

    fn tree_state(&self) -> RefMut<Option<TreeState>> {
        if self.tree_state.borrow().is_none() {
            self.tree_state.replace(Some(TreeState::load(
                self.store.clone(),
                self.working_copy_path.clone(),
                self.state_path.clone(),
            )));
        }
        self.tree_state.borrow_mut()
    }

    pub fn current_tree_id(&self) -> TreeId {
        self.tree_state()
            .as_ref()
            .unwrap()
            .current_tree_id()
            .clone()
    }

    pub fn file_states(&self) -> BTreeMap<RepoPath, FileState> {
        self.tree_state().as_ref().unwrap().file_states().clone()
    }

    fn save(&self) {
        let mut proto = crate::protos::working_copy::Checkout::new();
        proto.commit_id = self.current_commit_id().0;
        self.write_proto(proto);
    }

    pub fn check_out(&self, commit: Commit) -> Result<CheckoutStats, CheckoutError> {
        assert!(commit.is_open());
        let lock_path = self.state_path.join("working_copy.lock");
        let _lock = FileLock::lock(lock_path);

        // TODO: Write a "pending_checkout" file with the old and new TreeIds so we can
        // continue       an interrupted checkout if we find such a file. Write
        // access to that file can       also serve as lock so only one process
        // at once can do a checkout.

        // Check if the current checkout has changed on disk after we read it. It's safe
        // to check out another commit regardless, but it's probably not what
        // the caller wanted, so we let them know.
        //
        // We could safely add a version of this function without the check if we see a
        // need for it.
        let current_proto = self.read_proto();
        if let Some(commit_id_at_read_time) = self.commit_id.borrow().as_ref() {
            if current_proto.commit_id != commit_id_at_read_time.0 {
                return Err(CheckoutError::ConcurrentCheckout);
            }
        }

        let stats = self
            .tree_state()
            .as_mut()
            .unwrap()
            .check_out(commit.tree().id().clone())?;

        self.commit_id.replace(Some(commit.id().clone()));
        self.commit.replace(Some(commit));

        self.save();
        // TODO: Clear the "pending_checkout" file here.
        Ok(stats)
    }

    pub fn commit(
        &self,
        settings: &UserSettings,
        mut repo: Arc<ReadonlyRepo>,
    ) -> (Arc<ReadonlyRepo>, Commit) {
        let lock_path = self.state_path.join("working_copy.lock");
        let _lock = FileLock::lock(lock_path);

        // Check if the current checkout has changed on disk after we read it. It's fine
        // if it has, but we'll want our new commit to be a successor of the one
        // just created in that case, so we need to reset our state to have the new
        // commit id.
        let current_proto = self.read_proto();
        let maybe_previous_commit_id = self
            .commit_id
            .replace(Some(CommitId(current_proto.commit_id.clone())));
        match maybe_previous_commit_id {
            Some(previous_commit_id) if previous_commit_id.0 != current_proto.commit_id => {
                // Reload the repo so the new commit is visible in the index and view
                // TODO: This is not enough. The new commit is not necessarily still in the
                // view when we reload.
                repo = repo.reload();
            }
            _ => {}
        }
        let current_commit = self.current_commit();

        let new_tree_id = self.tree_state().as_mut().unwrap().write_tree().clone();
        if &new_tree_id != current_commit.tree().id() {
            let mut tx = repo.start_transaction("commit working copy");
            let mut_repo = tx.mut_repo();
            let commit = CommitBuilder::for_rewrite_from(settings, repo.store(), &current_commit)
                .set_tree(new_tree_id)
                .write_to_repo(mut_repo);
            mut_repo.set_checkout(commit.id().clone());
            repo = tx.commit();

            self.commit_id.replace(Some(commit.id().clone()));
            self.commit.replace(Some(commit));
            self.save();
        }
        let commit = self.commit.borrow().as_ref().unwrap().clone();
        (repo, commit)
    }
}
