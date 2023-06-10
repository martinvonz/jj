// Copyright 2020 The Jujutsu Authors
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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};

use itertools::Itertools;
use jujutsu_lib::backend::{Backend, BackendError, FileId, TreeId, TreeValue};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::git_backend::GitBackend;
use jujutsu_lib::local_backend::LocalBackend;
use jujutsu_lib::repo::{MutableRepo, ReadonlyRepo, Repo, RepoLoader, StoreFactories};
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::rewrite::RebasedDescendant;
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::store::Store;
use jujutsu_lib::tree::Tree;
use jujutsu_lib::tree_builder::TreeBuilder;
use jujutsu_lib::workspace::Workspace;
use tempfile::TempDir;

pub fn hermetic_libgit2() {
    // libgit2 respects init.defaultBranch (and possibly other config
    // variables) in the user's config files. Disable access to them to make
    // our tests hermetic.
    //
    // set_search_path is unsafe because it cannot guarantee thread safety (as
    // its documentation states). For the same reason, we wrap these invocations
    // in `call_once`.
    static CONFIGURE_GIT2: Once = Once::new();
    CONFIGURE_GIT2.call_once(|| unsafe {
        git2::opts::set_search_path(git2::ConfigLevel::System, "").unwrap();
        git2::opts::set_search_path(git2::ConfigLevel::Global, "").unwrap();
        git2::opts::set_search_path(git2::ConfigLevel::XDG, "").unwrap();
        git2::opts::set_search_path(git2::ConfigLevel::ProgramData, "").unwrap();
    });
}

pub fn new_temp_dir() -> TempDir {
    hermetic_libgit2();
    tempfile::Builder::new()
        .prefix("jj-test-")
        .tempdir()
        .unwrap()
}

pub fn user_settings() -> UserSettings {
    let config = config::Config::builder()
        .add_source(config::File::from_str(
            r#"
                user.name = "Test User"
                user.email = "test.user@example.com"
                operation.username = "test-username"
                operation.hostname = "host.example.com"
                debug.randomness-seed = "42"
           "#,
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap();
    UserSettings::from_config(config)
}

pub struct TestRepo {
    _temp_dir: TempDir,
    pub repo: Arc<ReadonlyRepo>,
}

impl TestRepo {
    pub fn init(use_git: bool) -> Self {
        let settings = user_settings();
        let temp_dir = new_temp_dir();

        let repo_dir = temp_dir.path().join("repo");
        fs::create_dir(&repo_dir).unwrap();

        let repo = if use_git {
            let git_path = temp_dir.path().join("git-repo");
            git2::Repository::init(&git_path).unwrap();
            ReadonlyRepo::init(
                &settings,
                &repo_dir,
                |store_path| -> Result<Box<dyn Backend>, BackendError> {
                    Ok(Box::new(GitBackend::init_external(store_path, &git_path)?))
                },
                ReadonlyRepo::default_op_store_factory(),
                ReadonlyRepo::default_op_heads_store_factory(),
                ReadonlyRepo::default_index_store_factory(),
                ReadonlyRepo::default_submodule_store_factory(),
            )
            .unwrap()
        } else {
            ReadonlyRepo::init(
                &settings,
                &repo_dir,
                |store_path| -> Result<Box<dyn Backend>, BackendError> {
                    Ok(Box::new(LocalBackend::init(store_path)))
                },
                ReadonlyRepo::default_op_store_factory(),
                ReadonlyRepo::default_op_heads_store_factory(),
                ReadonlyRepo::default_index_store_factory(),
                ReadonlyRepo::default_submodule_store_factory(),
            )
            .unwrap()
        };

        Self {
            _temp_dir: temp_dir,
            repo,
        }
    }
}

pub struct TestWorkspace {
    temp_dir: TempDir,
    pub workspace: Workspace,
    pub repo: Arc<ReadonlyRepo>,
}

impl TestWorkspace {
    pub fn init(settings: &UserSettings, use_git: bool) -> Self {
        let temp_dir = new_temp_dir();

        let workspace_root = temp_dir.path().join("repo");
        fs::create_dir(&workspace_root).unwrap();

        let (workspace, repo) = if use_git {
            let git_path = temp_dir.path().join("git-repo");
            git2::Repository::init(&git_path).unwrap();
            Workspace::init_external_git(settings, &workspace_root, &git_path).unwrap()
        } else {
            Workspace::init_local(settings, &workspace_root).unwrap()
        };

        Self {
            temp_dir,
            workspace,
            repo,
        }
    }

    pub fn root_dir(&self) -> PathBuf {
        self.temp_dir.path().join("repo").join("..")
    }
}

pub fn load_repo_at_head(settings: &UserSettings, repo_path: &Path) -> Arc<ReadonlyRepo> {
    RepoLoader::init(settings, repo_path, &StoreFactories::default())
        .unwrap()
        .load_at_head(settings)
        .unwrap()
}

pub fn read_file(store: &Store, path: &RepoPath, id: &FileId) -> Vec<u8> {
    let mut reader = store.read_file(path, id).unwrap();
    let mut content = vec![];
    reader.read_to_end(&mut content).unwrap();
    content
}

pub fn write_file(store: &Store, path: &RepoPath, contents: &str) -> FileId {
    store.write_file(path, &mut contents.as_bytes()).unwrap()
}

pub fn write_normal_file(tree_builder: &mut TreeBuilder, path: &RepoPath, contents: &str) {
    let id = write_file(tree_builder.store(), path, contents);
    tree_builder.set(
        path.clone(),
        TreeValue::File {
            id,
            executable: false,
        },
    );
}

pub fn write_executable_file(tree_builder: &mut TreeBuilder, path: &RepoPath, contents: &str) {
    let id = write_file(tree_builder.store(), path, contents);
    tree_builder.set(
        path.clone(),
        TreeValue::File {
            id,
            executable: true,
        },
    );
}

pub fn write_symlink(tree_builder: &mut TreeBuilder, path: &RepoPath, target: &str) {
    let id = tree_builder.store().write_symlink(path, target).unwrap();
    tree_builder.set(path.clone(), TreeValue::Symlink(id));
}

pub fn create_tree(repo: &Arc<ReadonlyRepo>, path_contents: &[(&RepoPath, &str)]) -> Tree {
    let store = repo.store();
    let mut tree_builder = store.tree_builder(store.empty_tree_id().clone());
    for (path, contents) in path_contents {
        write_normal_file(&mut tree_builder, path, contents);
    }
    let id = tree_builder.write_tree();
    store.get_tree(&RepoPath::root(), &id).unwrap()
}

#[must_use]
pub fn create_random_tree(repo: &Arc<ReadonlyRepo>) -> TreeId {
    let mut tree_builder = repo
        .store()
        .tree_builder(repo.store().empty_tree_id().clone());
    let number = rand::random::<u32>();
    let path = RepoPath::from_internal_string(format!("file{number}").as_str());
    write_normal_file(&mut tree_builder, &path, "contents");
    tree_builder.write_tree()
}

pub fn create_random_commit<'repo>(
    mut_repo: &'repo mut MutableRepo,
    settings: &UserSettings,
) -> CommitBuilder<'repo> {
    let tree_id = create_random_tree(mut_repo.base_repo());
    let number = rand::random::<u32>();
    mut_repo
        .new_commit(
            settings,
            vec![mut_repo.store().root_commit_id().clone()],
            tree_id,
        )
        .set_description(format!("random commit {number}"))
}

pub fn write_random_commit(mut_repo: &mut MutableRepo, settings: &UserSettings) -> Commit {
    create_random_commit(mut_repo, settings).write().unwrap()
}

pub fn write_working_copy_file(workspace_root: &Path, path: &RepoPath, contents: &str) {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path.to_fs_path(workspace_root))
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
        write_random_commit(self.mut_repo, self.settings)
    }

    pub fn commit_with_parents(&mut self, parents: &[&Commit]) -> Commit {
        let parent_ids = parents
            .iter()
            .map(|commit| commit.id().clone())
            .collect_vec();
        create_random_commit(self.mut_repo, self.settings)
            .set_parents(parent_ids)
            .write()
            .unwrap()
    }
}

pub fn assert_rebased(
    rebased: Option<RebasedDescendant>,
    expected_old_commit: &Commit,
    expected_new_parents: &[&Commit],
) -> Commit {
    if let Some(RebasedDescendant {
        old_commit,
        new_commit,
    }) = rebased
    {
        assert_eq!(old_commit, *expected_old_commit);
        assert_eq!(new_commit.change_id(), expected_old_commit.change_id());
        assert_eq!(
            new_commit.parent_ids(),
            expected_new_parents
                .iter()
                .map(|commit| commit.id().clone())
                .collect_vec()
        );
        new_commit
    } else {
        panic!("expected rebased commit: {rebased:?}");
    }
}
