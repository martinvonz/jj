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
use jujutsu_lib::backend::{Conflict, ConflictPart, TreeValue};
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::repo::ReadonlyRepo;
use jujutsu_lib::repo_path::{RepoPath, RepoPathComponent};
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::testutils;
use jujutsu_lib::tree_builder::TreeBuilder;
use test_case::test_case;

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_root(use_git: bool) {
    // Test that the working copy is clean and empty after init.
    let settings = testutils::user_settings();
    let mut test_workspace = testutils::init_workspace(&settings, use_git);
    let repo = &test_workspace.repo;

    let wc = test_workspace.workspace.working_copy_mut();
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc.write_tree();
    locked_wc.discard();
    let checkout_commit = repo.store().get_commit(repo.view().checkout()).unwrap();
    assert_eq!(&new_tree_id, checkout_commit.tree().id());
    assert_eq!(&new_tree_id, repo.store().empty_tree_id());
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_checkout_file_transitions(use_git: bool) {
    // Tests switching between commits where a certain path is of one type in one
    // commit and another type in the other. Includes a "missing" type, so we cover
    // additions and removals as well.

    let settings = testutils::user_settings();
    let mut test_workspace = testutils::init_workspace(&settings, use_git);
    let repo = &test_workspace.repo;
    let store = repo.store().clone();
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    #[derive(Debug, Clone, Copy)]
    enum Kind {
        Missing,
        Normal,
        Executable,
        Conflict,
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
            Kind::Conflict => {
                let base_file_id = testutils::write_file(
                    store,
                    &RepoPath::from_internal_string(path),
                    "base file contents",
                );
                let left_file_id = testutils::write_file(
                    store,
                    &RepoPath::from_internal_string(path),
                    "left file contents",
                );
                let right_file_id = testutils::write_file(
                    store,
                    &RepoPath::from_internal_string(path),
                    "right file contents",
                );
                let conflict = Conflict {
                    removes: vec![ConflictPart {
                        value: TreeValue::Normal {
                            id: base_file_id,
                            executable: false,
                        },
                    }],
                    adds: vec![
                        ConflictPart {
                            value: TreeValue::Normal {
                                id: left_file_id,
                                executable: false,
                            },
                        },
                        ConflictPart {
                            value: TreeValue::Normal {
                                id: right_file_id,
                                executable: false,
                            },
                        },
                    ],
                };
                let conflict_id = store.write_conflict(&conflict).unwrap();
                TreeValue::Conflict(conflict_id)
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
                let mut tx = repo.start_transaction("test");
                let id = testutils::create_random_commit(settings, repo)
                    .write_to_repo(tx.mut_repo())
                    .id()
                    .clone();
                tx.commit();
                TreeValue::GitSubmodule(id)
            }
        };
        tree_builder.set(RepoPath::from_internal_string(path), value);
    }

    let mut kinds = vec![
        Kind::Missing,
        Kind::Normal,
        Kind::Executable,
        Kind::Conflict,
        Kind::Tree,
    ];
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
            write_path(&settings, repo, &mut left_tree_builder, *left_kind, &path);
            write_path(&settings, repo, &mut right_tree_builder, *right_kind, &path);
            files.push((*left_kind, *right_kind, path));
        }
    }
    let left_tree_id = left_tree_builder.write_tree();
    let right_tree_id = right_tree_builder.write_tree();

    let mut tx = repo.start_transaction("test");
    let left_commit = CommitBuilder::for_new_commit(&settings, repo.store(), left_tree_id)
        .set_parents(vec![store.root_commit_id().clone()])
        .set_open(true)
        .write_to_repo(tx.mut_repo());
    let right_commit = CommitBuilder::for_new_commit(&settings, repo.store(), right_tree_id)
        .set_parents(vec![store.root_commit_id().clone()])
        .set_open(true)
        .write_to_repo(tx.mut_repo());
    let repo = tx.commit();

    let wc = test_workspace.workspace.working_copy_mut();
    wc.check_out(repo.op_id().clone(), None, left_commit)
        .unwrap();
    wc.check_out(repo.op_id().clone(), None, right_commit.clone())
        .unwrap();

    // Check that the working copy is clean.
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc.write_tree();
    locked_wc.discard();
    assert_eq!(&new_tree_id, right_commit.tree().id());

    for (_left_kind, right_kind, path) in &files {
        let wc_path = workspace_root.join(path);
        let maybe_metadata = wc_path.symlink_metadata();
        match right_kind {
            Kind::Missing => {
                assert!(maybe_metadata.is_err(), "{:?} should not exist", path);
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
            Kind::Conflict => {
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
                assert!(maybe_metadata.is_err(), "{:?} should not exist", path);
            }
        };
    }
}

#[test]
fn test_reset() {
    let settings = testutils::user_settings();
    let mut test_workspace = testutils::init_workspace(&settings, false);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let ignored_path = RepoPath::from_internal_string("ignored");
    let gitignore_path = RepoPath::from_internal_string(".gitignore");

    let tree_without_file = testutils::create_tree(repo, &[(&gitignore_path, "ignored\n")]);
    let tree_with_file = testutils::create_tree(
        repo,
        &[(&gitignore_path, "ignored\n"), (&ignored_path, "code")],
    );
    let mut tx = repo.start_transaction("test");
    let store = repo.store();
    let root_commit = store.root_commit_id();
    let commit_without_file = CommitBuilder::for_open_commit(
        &settings,
        store,
        root_commit.clone(),
        tree_without_file.id().clone(),
    )
    .write_to_repo(tx.mut_repo());
    let commit_with_file = CommitBuilder::for_open_commit(
        &settings,
        store,
        root_commit.clone(),
        tree_with_file.id().clone(),
    )
    .write_to_repo(tx.mut_repo());
    let repo = tx.commit();
    test_workspace.repo = repo.clone();

    let wc = test_workspace.workspace.working_copy_mut();
    wc.check_out(repo.op_id().clone(), None, commit_with_file.clone())
        .unwrap();

    // Test the setup: the file should exist on disk and in the tree state.
    assert!(ignored_path.to_fs_path(&workspace_root).is_file());
    assert!(wc.file_states().contains_key(&ignored_path));

    // After we reset to the commit without the file, it should still exist on disk,
    // but it should not be in the tree state, and it should not get added when we
    // commit the working copy (because it's ignored).
    let mut locked_wc = wc.start_mutation();
    locked_wc.reset(&tree_without_file).unwrap();
    locked_wc.finish(repo.op_id().clone(), commit_without_file.id().clone());
    assert!(ignored_path.to_fs_path(&workspace_root).is_file());
    assert!(!wc.file_states().contains_key(&ignored_path));
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc.write_tree();
    assert_eq!(new_tree_id, *tree_without_file.id());
    locked_wc.discard();

    // After we reset to the commit without the file, it should still exist on disk,
    // but it should not be in the tree state, and it should not get added when we
    // commit the working copy (because it's ignored).
    let mut locked_wc = wc.start_mutation();
    locked_wc.reset(&tree_without_file).unwrap();
    locked_wc.finish(repo.op_id().clone(), commit_without_file.id().clone());
    assert!(ignored_path.to_fs_path(&workspace_root).is_file());
    assert!(!wc.file_states().contains_key(&ignored_path));
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc.write_tree();
    assert_eq!(new_tree_id, *tree_without_file.id());
    locked_wc.discard();

    // Now test the opposite direction: resetting to a commit where the file is
    // tracked. The file should become tracked (even though it's ignored).
    let mut locked_wc = wc.start_mutation();
    locked_wc.reset(&tree_with_file).unwrap();
    locked_wc.finish(repo.op_id().clone(), commit_with_file.id().clone());
    assert!(ignored_path.to_fs_path(&workspace_root).is_file());
    assert!(wc.file_states().contains_key(&ignored_path));
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc.write_tree();
    assert_eq!(new_tree_id, *tree_with_file.id());
    locked_wc.discard();
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_commit_racy_timestamps(use_git: bool) {
    // Tests that file modifications are detected even if they happen the same
    // millisecond as the updated working copy state.
    let _home_dir = testutils::new_user_home();
    let settings = testutils::user_settings();
    let mut test_workspace = testutils::init_workspace(&settings, use_git);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let file_path = workspace_root.join("file");
    let mut previous_tree_id = repo.store().empty_tree_id().clone();
    let wc = test_workspace.workspace.working_copy_mut();
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
        let mut locked_wc = wc.start_mutation();
        let new_tree_id = locked_wc.write_tree();
        locked_wc.discard();
        assert_ne!(new_tree_id, previous_tree_id);
        previous_tree_id = new_tree_id;
    }
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_gitignores(use_git: bool) {
    // Tests that .gitignore files are respected.

    let _home_dir = testutils::new_user_home();
    let settings = testutils::user_settings();
    let mut test_workspace = testutils::init_workspace(&settings, use_git);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    let added_path = RepoPath::from_internal_string("added");
    let modified_path = RepoPath::from_internal_string("modified");
    let removed_path = RepoPath::from_internal_string("removed");
    let ignored_path = RepoPath::from_internal_string("ignored");
    let subdir_modified_path = RepoPath::from_internal_string("dir/modified");
    let subdir_ignored_path = RepoPath::from_internal_string("dir/ignored");

    testutils::write_working_copy_file(&workspace_root, &gitignore_path, "ignored\n");
    testutils::write_working_copy_file(&workspace_root, &modified_path, "1");
    testutils::write_working_copy_file(&workspace_root, &removed_path, "1");
    std::fs::create_dir(workspace_root.join("dir")).unwrap();
    testutils::write_working_copy_file(&workspace_root, &subdir_modified_path, "1");

    let wc = test_workspace.workspace.working_copy_mut();
    let mut locked_wc = wc.start_mutation();
    let new_tree_id1 = locked_wc.write_tree();
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

    testutils::write_working_copy_file(
        &workspace_root,
        &gitignore_path,
        "ignored\nmodified\nremoved\n",
    );
    testutils::write_working_copy_file(&workspace_root, &added_path, "2");
    testutils::write_working_copy_file(&workspace_root, &modified_path, "2");
    std::fs::remove_file(removed_path.to_fs_path(&workspace_root)).unwrap();
    testutils::write_working_copy_file(&workspace_root, &ignored_path, "2");
    testutils::write_working_copy_file(&workspace_root, &subdir_modified_path, "2");
    testutils::write_working_copy_file(&workspace_root, &subdir_ignored_path, "2");

    let mut locked_wc = wc.start_mutation();
    let new_tree_id2 = locked_wc.write_tree();
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

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_gitignores_checkout_overwrites_ignored(use_git: bool) {
    // Tests that a .gitignore'd file gets overwritten if check out a commit where
    // the file is tracked.

    let _home_dir = testutils::new_user_home();
    let settings = testutils::user_settings();
    let mut test_workspace = testutils::init_workspace(&settings, use_git);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    // Write an ignored file called "modified" to disk
    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    testutils::write_working_copy_file(&workspace_root, &gitignore_path, "modified\n");
    let modified_path = RepoPath::from_internal_string("modified");
    testutils::write_working_copy_file(&workspace_root, &modified_path, "garbage");

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
    let wc = test_workspace.workspace.working_copy_mut();
    wc.check_out(repo.op_id().clone(), None, commit).unwrap();

    // Check that the new contents are in the working copy
    let path = workspace_root.join("modified");
    assert!(path.is_file());
    let mut file = File::open(path).unwrap();
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"contents");

    // Check that the file is in the tree created by committing the working copy
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc.write_tree();
    locked_wc.discard();
    let new_tree = repo
        .store()
        .get_tree(&RepoPath::root(), &new_tree_id)
        .unwrap();
    assert!(new_tree
        .entry(&RepoPathComponent::from("modified"))
        .is_some());
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_gitignores_ignored_directory_already_tracked(use_git: bool) {
    // Tests that a .gitignore'd directory that already has a tracked file in it
    // does not get removed when committing the working directory.

    let _home_dir = testutils::new_user_home();
    let settings = testutils::user_settings();
    let mut test_workspace = testutils::init_workspace(&settings, use_git);
    let repo = &test_workspace.repo;

    // Add a .gitignore file saying to ignore the directory "ignored/"
    let gitignore_path = RepoPath::from_internal_string(".gitignore");
    testutils::write_working_copy_file(
        test_workspace.workspace.workspace_root(),
        &gitignore_path,
        "/ignored/\n",
    );
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
    let wc = test_workspace.workspace.working_copy_mut();
    wc.check_out(repo.op_id().clone(), None, commit).unwrap();

    // Check that the file is still in the tree created by committing the working
    // copy (that it didn't get removed because the directory is ignored)
    let mut locked_wc = wc.start_mutation();
    let new_tree_id = locked_wc.write_tree();
    locked_wc.discard();
    let new_tree = repo
        .store()
        .get_tree(&RepoPath::root(), &new_tree_id)
        .unwrap();
    assert!(new_tree.path_value(&file_path).is_some());
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_dotgit_ignored(use_git: bool) {
    // Tests that .git directories and files are always ignored (we could accept
    // them if the backend is not git).

    let _home_dir = testutils::new_user_home();
    let settings = testutils::user_settings();
    let mut test_workspace = testutils::init_workspace(&settings, use_git);
    let repo = &test_workspace.repo;
    let workspace_root = test_workspace.workspace.workspace_root().clone();

    // Test with a .git/ directory (with a file in, since we don't write empty
    // trees)
    let dotgit_path = workspace_root.join(".git");
    std::fs::create_dir(&dotgit_path).unwrap();
    testutils::write_working_copy_file(
        &workspace_root,
        &RepoPath::from_internal_string(".git/file"),
        "contents",
    );
    let mut locked_wc = test_workspace.workspace.working_copy_mut().start_mutation();
    let new_tree_id = locked_wc.write_tree();
    assert_eq!(new_tree_id, *repo.store().empty_tree_id());
    locked_wc.discard();
    std::fs::remove_dir_all(&dotgit_path).unwrap();

    // Test with a .git file
    testutils::write_working_copy_file(
        &workspace_root,
        &RepoPath::from_internal_string(".git"),
        "contents",
    );
    let mut locked_wc = test_workspace.workspace.working_copy_mut().start_mutation();
    let new_tree_id = locked_wc.write_tree();
    assert_eq!(new_tree_id, *repo.store().empty_tree_id());
    locked_wc.discard();
}
