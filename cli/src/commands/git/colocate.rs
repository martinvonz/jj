// Copyright 2024 The Jujutsu Authors
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

use jj_lib::backend::Backend;
use jj_lib::file_util::IoResultExt;
use jj_lib::git;
use jj_lib::repo::Repo;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error::internal_error;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::commands::git::maybe_add_gitignore;
use crate::ui::Ui;

/// Make the current workspace colocated
///
/// This has a similar effect to `jj git init --colocate`, except
/// it works on existing JJ repositories. The end result is a repo
/// where you can run Git commands as well as JJ commands.
///
/// Limitations:
///
/// - Currently only works in the primary workspace.
///
/// - Your repo must have been initialized by `jj git init/clone`. Other
///   configurations are not supported.
#[derive(clap::Args, Clone, Debug)]
pub struct GitColocateArgs {}

#[instrument(skip_all)]
pub fn cmd_git_colocate(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &GitColocateArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let workspace = workspace_command.workspace();

    if workspace_command.working_copy_shared_with_git() {
        return Err(user_error(format!(
            "Workspace '{}' is already colocated.",
            workspace.workspace_id().as_str()
        )));
    }

    let Some(git_backend) = workspace_command.git_backend() else {
        return Err(user_error(
            "This repo is not using the Git backend, so you cannot colocate a workspace.",
        ));
    };

    let dotgit_path = workspace.workspace_root().join(".git");
    if dotgit_path.exists() {
        return Err(user_error(format!(
            "Path {} already exists, cannot colocate this workspace.",
            dotgit_path.display()
        )));
    }

    let git_repo = git_backend.git_repo();
    if workspace.is_primary_workspace() {
        let common_dir = git_repo.common_dir();
        let old_path = workspace_command.repo_path().join("store").join("git");
        let common_dir_canon = common_dir.canonicalize().context(common_dir)?;
        let old_path_canon = old_path.canonicalize().ok();

        let mut config =
            gix::config::File::from_git_dir(common_dir.to_path_buf()).map_err(internal_error)?;
        let is_bare = config
            .value::<gix::config::Boolean>("core.bare")
            .map_or(false, |x| x.is_true());

        if old_path.is_symlink()
            || !old_path.is_dir()
            || Some(common_dir_canon) != old_path_canon
            || !is_bare
        {
            // TODO: hint to use --prefer-worktree when we add worktree support
            return Err(user_error(format!(
                "Unsupported Git repo setup for colocation: requires bare repo at {}",
                old_path.display()
            )));
        }

        config
            .set_raw_value(&"core.bare", "false")
            .map_err(internal_error)?;
        let config_path = common_dir.join("config");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(&config_path)
            .context(config_path)?;
        config
            .write_to(&mut file)
            .map_err(|e| internal_error(format!("Could not write git config file: {e}")))?;
        drop(file);

        let new_path = workspace.workspace_root().join(".git");
        fs::rename(&old_path, &new_path).context(old_path)?;
        let git_target_path = workspace_command
            .repo_path()
            .join("store")
            .join("git_target");
        fs::write(&git_target_path, "../../../.git").context(git_target_path)?;
        // we write .gitignore below
    } else {
        return Err(internal_error(
            "Unimplemented: colocating a secondary workspace",
        ));
    }

    // Both the Workspace (i.e. the store) and the WorkspaceCommandHelper
    // need to be reloaded to pick up changes to colocation.
    //
    // This way we end up with a git HEAD written immediately, rather than
    // next time @ moves. And you can immediately start running git commands.
    let workspace = workspace
        .reload(
            command.workspace_loader()?,
            command.settings(),
            command.get_working_copy_factory()?,
            command.get_store_factories(),
        )
        .map_err(internal_error)?;

    let repo = workspace.repo_loader().load_at_head(command.settings())?;
    let mut command = command.for_workable_repo(ui, workspace, repo)?;

    maybe_add_gitignore(&command)?;

    let Some(git_backend) = command.git_backend() else {
        return Err(internal_error("Reloaded repo no longer backed by git"));
    };
    let Some(wc_commit_id) = command.get_wc_commit_id() else {
        return Err(internal_error("Could not get the working copy"));
    };

    let name = command.workspace_id().as_str().to_owned();
    let wc_commit = command.repo().store().get_commit(wc_commit_id)?;

    if let Some(parent_id) = wc_commit.parent_ids().first() {
        if parent_id == git_backend.root_commit_id() {
            // No need to run reset_head, all it will do is show "Nothing changed"
            return Ok(());
        }
    }

    let git2_repo = git_backend.open_git_repo()?;
    let mut tx = command.start_transaction();

    git::reset_head(tx.repo_mut(), &git2_repo, &wc_commit)?;

    tx.finish(ui, format!("Colocated existing workspace {name}"))?;

    Ok(())
}
