// Copyright 2023 The Jujutsu Authors
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
use std::path::{Path, PathBuf};
use std::sync::Arc;

use itertools::Itertools;
use jj_cli::cli_util::{CliRunner, CommandError, CommandHelper};
use jj_cli::ui::Ui;
use jj_lib::backend::{Backend, MergedTreeId};
use jj_lib::commit::Commit;
use jj_lib::git_backend::GitBackend;
use jj_lib::local_working_copy::LocalWorkingCopy;
use jj_lib::merged_tree::MergedTree;
use jj_lib::op_store::{OperationId, WorkspaceId};
use jj_lib::repo::ReadonlyRepo;
use jj_lib::repo_path::RepoPath;
use jj_lib::settings::UserSettings;
use jj_lib::store::Store;
use jj_lib::working_copy::{
    CheckoutError, CheckoutStats, LockedWorkingCopy, ResetError, SnapshotError, SnapshotOptions,
    WorkingCopy, WorkingCopyStateError,
};
use jj_lib::workspace::{default_working_copy_factories, WorkingCopyInitializer, Workspace};

#[derive(clap::Parser, Clone, Debug)]
enum CustomCommands {
    /// Initialize a workspace using the "conflicts" working copy
    InitConflicts,
}

fn run_custom_command(
    _ui: &mut Ui,
    command_helper: &CommandHelper,
    command: CustomCommands,
) -> Result<(), CommandError> {
    match command {
        CustomCommands::InitConflicts => {
            let wc_path = command_helper.cwd();
            let backend_initializer = |settings: &UserSettings, store_path: &Path| {
                let backend: Box<dyn Backend> =
                    Box::new(GitBackend::init_internal(settings, store_path)?);
                Ok(backend)
            };
            Workspace::init_with_factories(
                command_helper.settings(),
                wc_path,
                &backend_initializer,
                &ReadonlyRepo::default_op_store_initializer(),
                &ReadonlyRepo::default_op_heads_store_initializer(),
                &ReadonlyRepo::default_index_store_initializer(),
                &ReadonlyRepo::default_submodule_store_initializer(),
                &ConflictsWorkingCopy::initializer(),
                WorkspaceId::default(),
            )?;
            Ok(())
        }
    }
}

fn main() -> std::process::ExitCode {
    let mut working_copy_factories = default_working_copy_factories();
    working_copy_factories.insert(
        ConflictsWorkingCopy::name().to_owned(),
        Box::new(|store, working_copy_path, state_path| {
            Box::new(ConflictsWorkingCopy::load(
                store.clone(),
                working_copy_path.to_owned(),
                state_path.to_owned(),
            ))
        }),
    );
    CliRunner::init()
        .set_working_copy_factories(working_copy_factories)
        .add_subcommand(run_custom_command)
        .run()
}

/// A working copy that adds a .conflicts file with a list of unresolved
/// conflicts.
///
/// Most functions below just delegate to the inner working-copy backend. The
/// only interesting functions are `snapshot()` and `check_out()`. The former
/// adds `.conflicts` to the .gitignores. The latter writes the `.conflicts`
/// file to the working copy.
struct ConflictsWorkingCopy {
    inner: Box<dyn WorkingCopy>,
}

impl ConflictsWorkingCopy {
    fn name() -> &'static str {
        "conflicts"
    }

    fn init(
        store: Arc<Store>,
        working_copy_path: PathBuf,
        state_path: PathBuf,
        workspace_id: WorkspaceId,
        operation_id: OperationId,
    ) -> Result<Self, WorkingCopyStateError> {
        let inner = LocalWorkingCopy::init(
            store,
            working_copy_path,
            state_path,
            operation_id,
            workspace_id,
        )?;
        Ok(ConflictsWorkingCopy {
            inner: Box::new(inner),
        })
    }

    fn initializer() -> Box<WorkingCopyInitializer> {
        Box::new(
            |store, working_copy_path, state_path, workspace_id, operation_id| {
                let wc = Self::init(
                    store,
                    working_copy_path,
                    state_path,
                    workspace_id,
                    operation_id,
                )?;
                Ok(Box::new(wc))
            },
        )
    }

    fn load(store: Arc<Store>, working_copy_path: PathBuf, state_path: PathBuf) -> Self {
        let inner = LocalWorkingCopy::load(store, working_copy_path, state_path);
        ConflictsWorkingCopy {
            inner: Box::new(inner),
        }
    }
}

impl WorkingCopy for ConflictsWorkingCopy {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        Self::name()
    }

    fn path(&self) -> &Path {
        self.inner.path()
    }

    fn workspace_id(&self) -> &WorkspaceId {
        self.inner.workspace_id()
    }

    fn operation_id(&self) -> &OperationId {
        self.inner.operation_id()
    }

    fn tree_id(&self) -> Result<&MergedTreeId, WorkingCopyStateError> {
        self.inner.tree_id()
    }

    fn sparse_patterns(&self) -> Result<&[RepoPath], WorkingCopyStateError> {
        self.inner.sparse_patterns()
    }

    fn start_mutation(&self) -> Result<Box<dyn LockedWorkingCopy>, WorkingCopyStateError> {
        let inner = self.inner.start_mutation()?;
        Ok(Box::new(LockedConflictsWorkingCopy {
            wc_path: self.inner.path().to_owned(),
            inner,
        }))
    }
}

struct LockedConflictsWorkingCopy {
    wc_path: PathBuf,
    inner: Box<dyn LockedWorkingCopy>,
}

impl LockedWorkingCopy for LockedConflictsWorkingCopy {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn old_operation_id(&self) -> &OperationId {
        self.inner.old_operation_id()
    }

    fn old_tree_id(&self) -> &MergedTreeId {
        self.inner.old_tree_id()
    }

    fn snapshot(&mut self, mut options: SnapshotOptions) -> Result<MergedTreeId, SnapshotError> {
        options.base_ignores = options.base_ignores.chain("", "/.conflicts".as_bytes());
        self.inner.snapshot(options)
    }

    fn check_out(&mut self, commit: &Commit) -> Result<CheckoutStats, CheckoutError> {
        let conflicts = commit
            .tree()?
            .conflicts()
            .map(|(path, _value)| format!("{}\n", path.to_internal_file_string()))
            .join("");
        std::fs::write(self.wc_path.join(".conflicts"), conflicts).unwrap();
        self.inner.check_out(commit)
    }

    fn reset(&mut self, new_tree: &MergedTree) -> Result<(), ResetError> {
        self.inner.reset(new_tree)
    }

    fn sparse_patterns(&self) -> Result<&[RepoPath], WorkingCopyStateError> {
        self.inner.sparse_patterns()
    }

    fn set_sparse_patterns(
        &mut self,
        new_sparse_patterns: Vec<RepoPath>,
    ) -> Result<CheckoutStats, CheckoutError> {
        self.inner.set_sparse_patterns(new_sparse_patterns)
    }

    fn finish(
        self: Box<Self>,
        operation_id: OperationId,
    ) -> Result<Box<dyn WorkingCopy>, WorkingCopyStateError> {
        let inner = self.inner.finish(operation_id)?;
        Ok(Box::new(ConflictsWorkingCopy { inner }))
    }
}
