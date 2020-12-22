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

#[cfg(not(windows))]
use std::os::unix::fs::PermissionsExt;

use jj_lib::commit_builder::CommitBuilder;
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::repo_path::{FileRepoPath, RepoPath};
use jj_lib::settings::UserSettings;
use jj_lib::store::TreeValue;
use jj_lib::testutils;
use jj_lib::tree_builder::TreeBuilder;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use test_case::test_case;

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_root(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let owned_wc = repo.working_copy().clone();
    let wc = owned_wc.lock().unwrap();
    assert_eq!(&wc.current_commit_id(), repo.view().checkout());
    assert_ne!(&wc.current_commit_id(), repo.store().root_commit_id());
    let wc_commit = wc.commit(&settings, Arc::get_mut(&mut repo).unwrap());
    assert_eq!(wc_commit.id(), repo.view().checkout());
    assert_eq!(wc_commit.tree().id(), repo.store().empty_tree_id());
    assert_eq!(wc_commit.store_commit().parents, vec![]);
    assert_eq!(wc_commit.predecessors(), vec![]);
    assert_eq!(wc_commit.description(), "");
    assert_eq!(wc_commit.is_open(), true);
    assert_eq!(wc_commit.author().name, settings.user_name());
    assert_eq!(wc_commit.author().email, settings.user_email());
    assert_eq!(wc_commit.committer().name, settings.user_name());
    assert_eq!(wc_commit.committer().email, settings.user_email());
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_checkout_file_transitions(use_git: bool) {
    // Tests switching between commits where a certain path is of one type in one
    // commit and another type in the other. Includes a "missing" type, so we cover
    // additions and removals as well.

    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store().clone();

    #[derive(Debug, Clone, Copy)]
    enum Kind {
        Missing,
        Normal,
        Executable,
        Symlink,
        Tree,
        GitSubmodule,
    };

    fn write_path(
        settings: &UserSettings,
        repo: &ReadonlyRepo,
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
                let id =
                    testutils::write_file(store, &FileRepoPath::from(path), "normal file contents");
                TreeValue::Normal {
                    id,
                    executable: false,
                }
            }
            Kind::Executable => {
                let id = testutils::write_file(
                    store,
                    &FileRepoPath::from(path),
                    "executable file contents",
                );
                TreeValue::Normal {
                    id,
                    executable: true,
                }
            }
            Kind::Symlink => {
                let id = store
                    .write_symlink(&FileRepoPath::from(path), "target")
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
                let id = testutils::create_random_commit(&settings, &repo)
                    .write_to_new_transaction(&repo, "test")
                    .id()
                    .clone();
                TreeValue::GitSubmodule(id)
            }
        };
        tree_builder.set(RepoPath::from(path), value);
    };

    let mut kinds = vec![
        Kind::Missing,
        Kind::Normal,
        Kind::Executable,
        Kind::Symlink,
        Kind::Tree,
    ];
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
    wc.commit(&settings, Arc::get_mut(&mut repo).unwrap());
    wc.check_out(right_commit.clone()).unwrap();

    // Check that the working copy is clean.
    let after_commit = wc.commit(&settings, Arc::get_mut(&mut repo).unwrap());
    let diff_summary = right_commit.tree().diff_summary(&after_commit.tree());
    assert_eq!(diff_summary.modified, vec![]);
    assert_eq!(diff_summary.added, vec![]);
    assert_eq!(diff_summary.removed, vec![]);

    for (_left_kind, right_kind, path) in &files {
        let wc_path = repo.working_copy_path().join(path);
        let maybe_metadata = wc_path.symlink_metadata();
        match right_kind {
            Kind::Missing => {
                assert_eq!(maybe_metadata.is_ok(), false, "{:?} should not exist", path);
            }
            Kind::Normal => {
                assert_eq!(maybe_metadata.is_ok(), true, "{:?} should exist", path);
                let metadata = maybe_metadata.unwrap();
                assert_eq!(metadata.is_file(), true, "{:?} should be a file", path);
                assert_eq!(
                    metadata.permissions().mode() & 0o111,
                    0,
                    "{:?} should not be executable",
                    path
                );
            }
            Kind::Executable => {
                assert_eq!(maybe_metadata.is_ok(), true, "{:?} should exist", path);
                let metadata = maybe_metadata.unwrap();
                assert_eq!(metadata.is_file(), true, "{:?} should be a file", path);
                assert_ne!(
                    metadata.permissions().mode() & 0o111,
                    0,
                    "{:?} should be executable",
                    path
                );
            }
            Kind::Symlink => {
                assert_eq!(maybe_metadata.is_ok(), true, "{:?} should exist", path);
                let metadata = maybe_metadata.unwrap();
                assert_eq!(
                    metadata.file_type().is_symlink(),
                    true,
                    "{:?} should be a symlink",
                    path
                );
            }
            Kind::Tree => {
                assert_eq!(maybe_metadata.is_ok(), true, "{:?} should exist", path);
                let metadata = maybe_metadata.unwrap();
                assert_eq!(metadata.is_dir(), true, "{:?} should be a directory", path);
            }
            Kind::GitSubmodule => {
                // Not supported for now
                assert_eq!(maybe_metadata.is_ok(), false, "{:?} should not exist", path);
            }
        };
    }
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_commit_racy_timestamps(use_git: bool) {
    // Tests that file modifications are detected even if they happen the same
    // millisecond as the updated working copy state.

    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

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
        let commit = wc.commit(&settings, Arc::get_mut(&mut repo).unwrap());
        let new_tree_id = commit.tree().id().clone();
        assert_ne!(new_tree_id, previous_tree_id);
        previous_tree_id = new_tree_id;
    }
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_gitignores(use_git: bool) {
    // Tests that .gitignore files are respected.

    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let gitignore_path = FileRepoPath::from(".gitignore");
    let added_path = FileRepoPath::from("added");
    let modified_path = FileRepoPath::from("modified");
    let removed_path = FileRepoPath::from("removed");
    let ignored_path = FileRepoPath::from("ignored");
    let subdir_modified_path = FileRepoPath::from("dir/modified");
    let subdir_ignored_path = FileRepoPath::from("dir/ignored");

    testutils::write_working_copy_file(&repo, &gitignore_path, "ignored\n");
    testutils::write_working_copy_file(&repo, &modified_path, "1");
    testutils::write_working_copy_file(&repo, &removed_path, "1");
    std::fs::create_dir(repo.working_copy_path().join("dir")).unwrap();
    testutils::write_working_copy_file(&repo, &subdir_modified_path, "1");

    let wc = repo.working_copy().clone();
    let commit1 = wc
        .lock()
        .unwrap()
        .commit(&settings, Arc::get_mut(&mut repo).unwrap());
    let files1: Vec<_> = commit1
        .tree()
        .entries()
        .map(|(name, _value)| name)
        .collect();
    assert_eq!(
        files1,
        vec![
            gitignore_path.to_repo_path(),
            subdir_modified_path.to_repo_path(),
            modified_path.to_repo_path(),
            removed_path.to_repo_path()
        ]
    );

    testutils::write_working_copy_file(&repo, &gitignore_path, "ignored\nmodified\nremoved\n");
    testutils::write_working_copy_file(&repo, &added_path, "2");
    testutils::write_working_copy_file(&repo, &modified_path, "2");
    std::fs::remove_file(
        repo.working_copy_path()
            .join(removed_path.to_internal_string()),
    )
    .unwrap();
    testutils::write_working_copy_file(&repo, &ignored_path, "2");
    testutils::write_working_copy_file(&repo, &subdir_modified_path, "2");
    testutils::write_working_copy_file(&repo, &subdir_ignored_path, "2");

    let wc = repo.working_copy().clone();
    let commit2 = wc
        .lock()
        .unwrap()
        .commit(&settings, Arc::get_mut(&mut repo).unwrap());
    let files2: Vec<_> = commit2
        .tree()
        .entries()
        .map(|(name, _value)| name)
        .collect();
    assert_eq!(
        files2,
        vec![
            gitignore_path.to_repo_path(),
            added_path.to_repo_path(),
            subdir_modified_path.to_repo_path(),
            modified_path.to_repo_path()
        ]
    );
}
