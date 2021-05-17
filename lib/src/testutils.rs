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

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;

use tempfile::TempDir;

use crate::commit::Commit;
use crate::commit_builder::CommitBuilder;
use crate::repo::{MutableRepo, ReadonlyRepo};
use crate::repo_path::{DirRepoPath, RepoPath};
use crate::settings::UserSettings;
use crate::store::{FileId, TreeId, TreeValue};
use crate::store_wrapper::StoreWrapper;
use crate::tree::Tree;
use crate::tree_builder::TreeBuilder;

pub fn new_user_home() -> TempDir {
    // Set $HOME to some arbitrary place so libgit2 doesn't use ~/.gitignore
    // of the person running the tests.
    let home_dir = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", home_dir.path());
    home_dir
}

pub fn user_settings() -> UserSettings {
    let mut config = config::Config::new();
    config.set("user.name", "Test User").unwrap();
    config.set("user.email", "test.user@example.com").unwrap();
    UserSettings::from_config(config)
}

pub fn init_repo(settings: &UserSettings, use_git: bool) -> (TempDir, Arc<ReadonlyRepo>) {
    let temp_dir = tempfile::tempdir().unwrap();

    let wc_path = temp_dir.path().join("repo");
    fs::create_dir(&wc_path).unwrap();

    let repo = if use_git {
        let git_path = temp_dir.path().join("git-repo");
        git2::Repository::init(&git_path).unwrap();
        ReadonlyRepo::init_external_git(&settings, wc_path, git_path).unwrap()
    } else {
        ReadonlyRepo::init_local(&settings, wc_path).unwrap()
    };

    (temp_dir, repo)
}

pub fn write_file(store: &StoreWrapper, path: &RepoPath, contents: &str) -> FileId {
    store.write_file(path, &mut contents.as_bytes()).unwrap()
}

pub fn write_normal_file(tree_builder: &mut TreeBuilder, path: &RepoPath, contents: &str) {
    let id = write_file(tree_builder.repo(), path, contents);
    tree_builder.set(
        path.clone(),
        TreeValue::Normal {
            id,
            executable: false,
        },
    );
}

pub fn write_executable_file(tree_builder: &mut TreeBuilder, path: &RepoPath, contents: &str) {
    let id = write_file(tree_builder.repo(), path, contents);
    tree_builder.set(
        path.clone(),
        TreeValue::Normal {
            id,
            executable: true,
        },
    );
}

pub fn write_symlink(tree_builder: &mut TreeBuilder, path: &RepoPath, target: &str) {
    let id = tree_builder.repo().write_symlink(path, target).unwrap();
    tree_builder.set(path.clone(), TreeValue::Symlink(id));
}

pub fn create_tree(repo: &ReadonlyRepo, path_contents: &[(&RepoPath, &str)]) -> Tree {
    let store = repo.store();
    let mut tree_builder = store.tree_builder(store.empty_tree_id().clone());
    for (path, contents) in path_contents {
        write_normal_file(&mut tree_builder, path, contents);
    }
    let id = tree_builder.write_tree();
    store.get_tree(&DirRepoPath::root(), &id).unwrap()
}

#[must_use]
pub fn create_random_tree(repo: &ReadonlyRepo) -> TreeId {
    let mut tree_builder = repo
        .store()
        .tree_builder(repo.store().empty_tree_id().clone());
    let number = rand::random::<u32>();
    let path = RepoPath::from_internal_string(format!("file{}", number).as_str());
    write_normal_file(&mut tree_builder, &path, "contents");
    tree_builder.write_tree()
}

#[must_use]
pub fn create_random_commit(settings: &UserSettings, repo: &ReadonlyRepo) -> CommitBuilder {
    let tree_id = create_random_tree(repo);
    let number = rand::random::<u32>();
    CommitBuilder::for_new_commit(settings, repo.store(), tree_id)
        .set_description(format!("random commit {}", number))
}

pub fn write_working_copy_file(repo: &ReadonlyRepo, path: &RepoPath, contents: &str) {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path.to_fs_path(repo.working_copy_path()))
        .unwrap();
    file.write_all(contents.as_bytes()).unwrap();
}

pub struct CommitGraphBuilder<'settings, 'repo> {
    settings: &'settings UserSettings,
    mut_repo: &'repo mut MutableRepo,
}

impl<'settings, 'repo> CommitGraphBuilder<'settings, 'repo> {
    pub fn new(
        settings: &'settings UserSettings,
        mut_repo: &'repo mut MutableRepo,
    ) -> CommitGraphBuilder<'settings, 'repo> {
        CommitGraphBuilder { settings, mut_repo }
    }

    pub fn initial_commit(&mut self) -> Commit {
        create_random_commit(self.settings, self.mut_repo.base_repo().as_ref())
            .write_to_repo(self.mut_repo)
    }

    pub fn commit_with_parents(&mut self, parents: &[&Commit]) -> Commit {
        let parent_ids: Vec<_> = parents.iter().map(|commit| commit.id().clone()).collect();
        create_random_commit(self.settings, self.mut_repo.base_repo().as_ref())
            .set_parents(parent_ids)
            .write_to_repo(self.mut_repo)
    }
}
