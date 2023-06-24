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

use std::any::Any;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use jujutsu::cli_util::{CliRunner, CommandError, CommandHelper};
use jujutsu::ui::Ui;
use jujutsu_lib::backend::{
    Backend, BackendResult, ChangeId, Commit, CommitId, Conflict, ConflictId, FileId, Signer,
    SymlinkId, Tree, TreeId,
};
use jujutsu_lib::git_backend::GitBackend;
use jujutsu_lib::repo::StoreFactories;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::workspace::Workspace;

#[derive(clap::Parser, Clone, Debug)]
enum CustomCommands {
    /// Initialize a workspace using the Jit backend
    InitJit,
}

fn create_store_factories() -> StoreFactories {
    let mut store_factories = StoreFactories::default();
    // Register the backend so it can be loaded when the repo is loaded. The name
    // must match `Backend::name()`.
    store_factories.add_backend(
        "jit",
        Box::new(|store_path| Box::new(JitBackend::load(store_path))),
    );
    store_factories
}

fn run_custom_command(
    _ui: &mut Ui,
    command_helper: &CommandHelper,
    command: CustomCommands,
) -> Result<(), CommandError> {
    match command {
        CustomCommands::InitJit => {
            let wc_path = command_helper.cwd();
            // Initialize a workspace with the custom backend
            Workspace::init_with_backend(command_helper.settings(), wc_path, |store_path| {
                Ok(Box::new(JitBackend::init(store_path)))
            })?;
            Ok(())
        }
    }
}

fn main() -> std::process::ExitCode {
    CliRunner::init()
        .set_store_factories(create_store_factories())
        .add_subcommand(run_custom_command)
        .run()
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
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "jit"
    }

    fn commit_id_length(&self) -> usize {
        self.inner.commit_id_length()
    }

    fn change_id_length(&self) -> usize {
        self.inner.change_id_length()
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

    fn root_change_id(&self) -> &ChangeId {
        self.inner.root_change_id()
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

    fn write_commit(&self, contents: Commit) -> BackendResult<(CommitId, Commit)> {
        self.inner.write_commit(contents)
    }

    fn install_signer(&self, signer: Arc<Signer>) -> BackendResult<()> {
        self.inner.install_signer(signer)
    }
}
