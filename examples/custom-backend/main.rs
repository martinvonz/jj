// Copyright 2022 The Jujutsu Authors
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

use std::io::Read;
use std::path::Path;

use clap::{FromArgMatches, Subcommand};
use git2::Repository;
use jujutsu::cli_util::{create_ui, handle_command_result, parse_args, CommandError};
use jujutsu::commands::{default_app, run_command};
use jujutsu::ui::Ui;
use jujutsu_lib::backend::{
    Backend, BackendResult, Commit, CommitId, Conflict, ConflictId, FileId, SymlinkId, Tree, TreeId,
};
use jujutsu_lib::git_backend::GitBackend;
use jujutsu_lib::repo::BackendFactories;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::workspace::Workspace;

#[derive(clap::Parser, Clone, Debug)]
enum CustomCommands {
    /// Initialize a workspace using the Jit backend
    InitJit,
}

fn run(ui: &mut Ui) -> Result<(), CommandError> {
    let app = CustomCommands::augment_subcommands(default_app());
    let (mut command_helper, matches) = parse_args(ui, app, std::env::args_os())?;
    let mut backend_factories = BackendFactories::default();
    // Register the backend so it can be loaded when the repo is loaded. The name
    // must match `Backend::name()`.
    backend_factories.add_backend(
        "jit",
        Box::new(|store_path| Box::new(JitBackend::load(store_path))),
    );
    command_helper.set_backend_factories(backend_factories);
    match CustomCommands::from_arg_matches(&matches) {
        // Handle our custom command
        Ok(CustomCommands::InitJit) => {
            let wc_path = ui.cwd();
            // Initialize a workspace with the custom backend
            Workspace::init_with_backend(ui.settings(), wc_path, |store_path| {
                Box::new(JitBackend::init(store_path))
            })?;
            Ok(())
        }
        // Handle default commands
        Err(_) => run_command(ui, &command_helper, &matches),
    }
}

fn main() {
    jujutsu::cleanup_guard::init();
    let (mut ui, result) = create_ui();
    let result = result.and_then(|()| run(&mut ui));
    let exit_code = handle_command_result(&mut ui, result);
    ui.finalize_writes();
    std::process::exit(exit_code);
}

/// A commit backend that's extremely similar to the Git backend
#[derive(Debug)]
struct JitBackend {
    inner: GitBackend,
}

impl JitBackend {
    fn init(store_path: &Path) -> Self {
        JitBackend {
            inner: GitBackend::init_internal(store_path),
        }
    }

    fn load(store_path: &Path) -> Self {
        JitBackend {
            inner: GitBackend::load(store_path),
        }
    }
}

impl Backend for JitBackend {
    fn name(&self) -> &str {
        "jit"
    }

    fn hash_length(&self) -> usize {
        self.inner.hash_length()
    }

    fn git_repo(&self) -> Option<Repository> {
        self.inner.git_repo()
    }

    fn read_file(&self, path: &RepoPath, id: &FileId) -> BackendResult<Box<dyn Read>> {
        self.inner.read_file(path, id)
    }

    fn write_file(&self, path: &RepoPath, contents: &mut dyn Read) -> BackendResult<FileId> {
        self.inner.write_file(path, contents)
    }

    fn read_symlink(&self, path: &RepoPath, id: &SymlinkId) -> BackendResult<String> {
        self.inner.read_symlink(path, id)
    }

    fn write_symlink(&self, path: &RepoPath, target: &str) -> BackendResult<SymlinkId> {
        self.inner.write_symlink(path, target)
    }

    fn root_commit_id(&self) -> &CommitId {
        self.inner.root_commit_id()
    }

    fn empty_tree_id(&self) -> &TreeId {
        self.inner.empty_tree_id()
    }

    fn read_tree(&self, path: &RepoPath, id: &TreeId) -> BackendResult<Tree> {
        self.inner.read_tree(path, id)
    }

    fn write_tree(&self, path: &RepoPath, contents: &Tree) -> BackendResult<TreeId> {
        self.inner.write_tree(path, contents)
    }

    fn read_conflict(&self, path: &RepoPath, id: &ConflictId) -> BackendResult<Conflict> {
        self.inner.read_conflict(path, id)
    }

    fn write_conflict(&self, path: &RepoPath, contents: &Conflict) -> BackendResult<ConflictId> {
        self.inner.write_conflict(path, contents)
    }

    fn read_commit(&self, id: &CommitId) -> BackendResult<Commit> {
        self.inner.read_commit(id)
    }

    fn write_commit(&self, contents: &Commit) -> BackendResult<CommitId> {
        self.inner.write_commit(contents)
    }
}
