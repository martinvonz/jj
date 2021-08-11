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

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use itertools::Itertools;
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::repo::ReadonlyRepo;
use jujutsu_lib::repo_path::{RepoPath, RepoPathComponent};
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::store::TreeValue;
use jujutsu_lib::testutils;
use jujutsu_lib::tree_builder::TreeBuilder;
use test_case::test_case;

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_root(use_git: bool) {
    // Test that the working copy is clean and empty after init.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let owned_wc = repo.working_copy().clone();
    let wc = owned_wc.lock().unwrap();
    let locked_wc = wc.write_tree();
    let new_tree_id = locked_wc.new_tree_id();
    locked_wc.discard();
    let checkout_commit = repo.store().get_commit(repo.view().checkout()).unwrap();
    assert_eq!(&new_tree_id, checkout_commit.tree().id());
    assert_eq!(&new_tree_id, repo.store().empty_tree_id());
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_checkout_file_transitions(use_git: bool) {
    // Tests switching between commits where a certain path is of one type in one
    // commit and another type in the other. Includes a "missing" type, so we cover
    // additions and removals as well.

    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store().clone();

    #[derive(Debug, Clone, Copy)]
    enum Kind {
        Missing,
        Normal,
        Executable,
        #[cfg_attr(windows, allow(dead_code))]
        Symlink,
        Tree,
        GitSubmodule,
    }

    fn write_path(
        settings: &UserSettings,
        repo: &Arc<ReadonlyRepo>,
        tree_builder: &mut TreeBuilder,
        kind: Kind,
        path: &str,
    ) {
        let store = repo.store();
        let value = match kind {
            Kind::Missing => {
                return;
            }
            Kind::Normal => {
                let id = testutils::write_file(
                    store,
                    &RepoPath::from_internal_string(path),
                    "normal file contents",
                );
                TreeValue::Normal {
                    id,
                    executable: false,
                }
            }
            Kind::Executable => {
                let id = testutils::write_file(
                    store,
                    &RepoPath::from_internal_string(path),
                    "executable file contents",
                );
                TreeValue::Normal {
                    id,
                    executable: true,
                }
            }
            Kind::Symlink => {
                let id = store
                    .write_symlink(&RepoPath::from_internal_string(path), "target")
                    .unwrap();
                TreeValue::Symlink(id)
            }
            Kind::Tree => {
                let mut sub_tree_builder = store.tree_builder(store.empty_tree_id().clone());
                let file_path = path.to_owned() + "/file";
                write_path(
                    settings,
                    repo,
                    &mut sub_tree_builder,
                    Kind::Normal,
                    &file_path,
                );
                let id = sub_tree_builder.write_tree();
                TreeValue::Tree(id)
            }
            Kind::GitSubmodule => {
                let id = testutils::create_random_commit(settings, repo)
                    .write_to_new_transaction(repo, "test")
                    .id()
                    .clone();
                TreeValue::GitSubmodule(id)
            }
        };
        tree_builder.set(RepoPath::from_internal_string(path), value);
    }

    let mut kinds = vec![Kind::Missing, Kind::Normal, Kind::Executable, Kind::Tree];
    #[cfg(unix)]
    kinds.push(Kind::Symlink);
    if use_git {
        kinds.push(Kind::GitSubmodule);
    }
    let mut left_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    let mut right_tree_builder = store.tree_builder(store.empty_tree_id().clone());
    let mut files = vec![];
    for left_kind in &kinds {
        for right_kind in &kinds {
            let path = format!("{:?}_{:?}", left_kind, right_kind);
            write_path(&settings, &repo, &mut left_tree_builder, *left_kind, &path);
            write_path(
                &settings,
                &repo,
                &mut right_tree_builder,
                *right_kind,
                &path,
            );
            files.push((*left_kind, *right_kind, path));
        }
    }
    let left_tree_id = left_tree_builder.write_tree();
    let right_tree_id = right_tree_builder.write_tree();

    let left_commit = CommitBuilder::for_new_commit(&settings, repo.store(), left_tree_id)
        .set_parents(vec![store.root_commit_id().clone()])
        .set_open(true)
        .write_to_new_transaction(&repo, "test");
    let right_commit = CommitBuilder::for_new_commit(&settings, repo.store(), right_tree_id)
        .set_parents(vec![store.root_commit_id().clone()])
        .set_open(true)
        .write_to_new_transaction(&repo, "test");

    let owned_wc = repo.working_copy().clone();
    let wc = owned_wc.lock().unwrap();
    wc.check_out(left_commit).unwrap();
    wc.check_out(right_commit.clone()).unwrap();

    // Check that the working copy is clean.
    let locked_wc = wc.write_tree();
    let new_tree_id = locked_wc.new_tree_id();
    locked_wc.discard();
    assert_eq!(&new_tree_id, right_commit.tree().id());

    for (_left_kind, right_kind, path) in &files {
        let wc_path = repo.working_copy_path().join(path);
        let maybe_metadata = wc_path.symlink_metadata();
        match right_kind {
            Kind::Missing => {
                assert!(!maybe_metadata.is_ok(), "{:?} should not exist", path);
            }
            Kind::Normal => {
                assert!(maybe_metadata.is_ok(), "{:?} should exist", path);
                let metadata = maybe_metadata.unwrap();
                assert!(metadata.is_file(), "{:?} should be a file", path);
                #[cfg(unix)]
                assert_eq!(
                    metadata.permissions().mode() & 0o111,
                    0,
                    "{:?} should not be executable",
                    path
                );
            }
            Kind::Executable => {
                assert!(maybe_metadata.is_ok(), "{:?} should exist", path);
                let metadata = maybe_metadata.unwrap();
                assert!(metadata.is_file(), "{:?} should be a file", path);
                #[cfg(unix)]
                assert_ne!(
                    metadata.permissions().mode() & 0o111,
                    0,
                    "{:?} should be executable",
                    path
                );
            }
            Kind::Symlink => {
                assert!(maybe_metadata.is_ok(), "{:?} should exist", path);
                let metadata = maybe_metadata.unwrap();
                assert!(
                    metadata.file_type().is_symlink(),
                    "{:?} should be a symlink",
                    path
                );
            }
            Kind::Tree => {
                assert!(maybe_metadata.is_ok(), "{:?} should exist", path);
                let metadata = maybe_metadata.unwrap();
                assert!(metadata.is_dir(), "{:?} should be a directory", path);
            }
            Kind::GitSubmodule => {
                // Not supported for now
                assert!(!maybe_metadata.is_ok(), "{:?} should not exist", path);
            }
        };
    }
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_commit_racy_timestamps(use_git: bool) {
    // Tests that file modifications are detected even if they happen the same
    // millisecond as the updated working copy state.
    let _home_dir = testutils::new_user_home();
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let file_path = repo.working_copy_path().join("file");
    let mut previous_tree_id = repo.store().empty_tree_id().clone();
    let owned_wc = repo.working_copy().clone();
    let wc = owned_wc.lock().unwrap();
    for i in 0..100 {
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .open(&file_path)
                .unwrap();
            file.write_all(format!("contents {}", i).as_bytes())
                .unwrap();
        }
        let locked_wc = wc.write_tree();
        let new_tree_id = locked_wc.new_tree_id();
        locked_wc.discard();
        assert_ne!(new_tree_id, previous_tree_id);
        previous_tree_id = new_tree_id;
    }
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_gitignores(use_git: bool) {
    // Tests that .gitignore files are respected.

    let _home_dir = testutils::new_user_home();
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    let added_path = RepoPath::from_internal_string("added");
    let modified_path = RepoPath::from_internal_string("modified");
    let removed_path = RepoPath::from_internal_string("removed");
    let ignored_path = RepoPath::from_internal_string("ignored");
    let subdir_modified_path = RepoPath::from_internal_string("dir/modified");
    let subdir_ignored_path = RepoPath::from_internal_string("dir/ignored");

    testutils::write_working_copy_file(&repo, &gitignore_path, "ignored\n");
    testutils::write_working_copy_file(&repo, &modified_path, "1");
    testutils::write_working_copy_file(&repo, &removed_path, "1");
    std::fs::create_dir(repo.working_copy_path().join("dir")).unwrap();
    testutils::write_working_copy_file(&repo, &subdir_modified_path, "1");

    let owned_wc = repo.working_copy().clone();
    let wc = owned_wc.lock().unwrap();
    let locked_wc = wc.write_tree();
    let new_tree_id1 = locked_wc.new_tree_id();
    locked_wc.discard();
    let tree1 = repo
        .store()
        .get_tree(&RepoPath::root(), &new_tree_id1)
        .unwrap();
    let files1 = tree1.entries().map(|(name, _value)| name).collect_vec();
    assert_eq!(
        files1,
        vec![
            gitignore_path.clone(),
            subdir_modified_path.clone(),
            modified_path.clone(),
            removed_path.clone(),
        ]
    );

    testutils::write_working_copy_file(&repo, &gitignore_path, "ignored\nmodified\nremoved\n");
    testutils::write_working_copy_file(&repo, &added_path, "2");
    testutils::write_working_copy_file(&repo, &modified_path, "2");
    std::fs::remove_file(removed_path.to_fs_path(repo.working_copy_path())).unwrap();
    testutils::write_working_copy_file(&repo, &ignored_path, "2");
    testutils::write_working_copy_file(&repo, &subdir_modified_path, "2");
    testutils::write_working_copy_file(&repo, &subdir_ignored_path, "2");

    let locked_wc = wc.write_tree();
    let new_tree_id2 = locked_wc.new_tree_id();
    locked_wc.discard();
    let tree2 = repo
        .store()
        .get_tree(&RepoPath::root(), &new_tree_id2)
        .unwrap();
    let files2 = tree2.entries().map(|(name, _value)| name).collect_vec();
    assert_eq!(
        files2,
        vec![
            gitignore_path,
            added_path,
            subdir_modified_path,
            modified_path,
        ]
    );
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_gitignores_checkout_overwrites_ignored(use_git: bool) {
    // Tests that a .gitignore'd file gets overwritten if check out a commit where
    // the file is tracked.

    let _home_dir = testutils::new_user_home();
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    // Write an ignored file called "modified" to disk
    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    testutils::write_working_copy_file(&repo, &gitignore_path, "modified\n");
    let modified_path = RepoPath::from_internal_string("modified");
    testutils::write_working_copy_file(&repo, &modified_path, "garbage");

    // Create a commit that adds the same file but with different contents
    let mut tx = repo.start_transaction("test");
    let mut tree_builder = repo
        .store()
        .tree_builder(repo.store().empty_tree_id().clone());
    testutils::write_normal_file(&mut tree_builder, &modified_path, "contents");
    let tree_id = tree_builder.write_tree();
    let commit = CommitBuilder::for_new_commit(&settings, repo.store(), tree_id)
        .set_open(true)
        .set_description("add file".to_string())
        .write_to_repo(tx.mut_repo());
    let repo = tx.commit();

    // Now check out the commit that adds the file "modified" with contents
    // "contents". The exiting contents ("garbage") should be replaced in the
    // working copy.
    repo.working_copy_locked().check_out(commit).unwrap();

    // Check that the new contents are in the working copy
    let path = repo.working_copy_path().join("modified");
    assert!(path.is_file());
    let mut file = File::open(path).unwrap();
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"contents");

    // Check that the file is in the tree created by committing the working copy
    let wc = repo.working_copy_locked();
    let locked_wc = wc.write_tree();
    let new_tree_id = locked_wc.new_tree_id();
    locked_wc.discard();
    let new_tree = repo
        .store()
        .get_tree(&RepoPath::root(), &new_tree_id)
        .unwrap();
    assert!(new_tree
        .entry(&RepoPathComponent::from("modified"))
        .is_some());
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_gitignores_ignored_directory_already_tracked(use_git: bool) {
    // Tests that a .gitignore'd directory that already has a tracked file in it
    // does not get removed when committing the working directory.

    let _home_dir = testutils::new_user_home();
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    // Add a .gitignore file saying to ignore the directory "ignored/"
    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    testutils::write_working_copy_file(&repo, &gitignore_path, "/ignored/\n");
    let file_path = RepoPath::from_internal_string("ignored/file");

    // Create a commit that adds a file in the ignored directory
    let mut tx = repo.start_transaction("test");
    let mut tree_builder = repo
        .store()
        .tree_builder(repo.store().empty_tree_id().clone());
    testutils::write_normal_file(&mut tree_builder, &file_path, "contents");
    let tree_id = tree_builder.write_tree();
    let commit = CommitBuilder::for_new_commit(&settings, repo.store(), tree_id)
        .set_open(true)
        .set_description("add ignored file".to_string())
        .write_to_repo(tx.mut_repo());
    let repo = tx.commit();

    // Check out the commit with the file in ignored/
    repo.working_copy_locked().check_out(commit).unwrap();

    // Check that the file is still in the tree created by committing the working
    // copy (that it didn't get removed because the directory is ignored)
    let wc = repo.working_copy_locked();
    let locked_wc = wc.write_tree();
    let new_tree_id = locked_wc.new_tree_id();
    locked_wc.discard();
    let new_tree = repo
        .store()
        .get_tree(&RepoPath::root(), &new_tree_id)
        .unwrap();
    assert!(new_tree.path_value(&file_path).is_some());
}
