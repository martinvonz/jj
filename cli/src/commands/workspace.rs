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

use std::fmt::Debug;
use std::fs;
use std::io::Write;
use std::sync::Arc;

use clap::Subcommand;
use itertools::Itertools;
use jj_lib::file_util;
use jj_lib::object_id::ObjectId;
use jj_lib::op_store::{OpStoreError, WorkspaceId};
use jj_lib::operation::Operation;
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::rewrite::merge_commit_trees;
use jj_lib::workspace::Workspace;
use tracing::instrument;

use crate::cli_util::{
    check_stale_working_copy, print_checkout_stats,
    resolve_multiple_nonempty_revsets_default_single, short_commit_hash, CommandHelper,
    RevisionArg, WorkingCopyFreshness, WorkspaceCommandHelper,
};
use crate::command_error::{internal_error_with_message, user_error, CommandError};
use crate::ui::Ui;

/// Commands for working with workspaces
///
/// Workspaces let you add additional working copies attached to the same repo.
/// A common use case is so you can run a slow build or test in one workspace
/// while you're continuing to write code in another workspace.
///
/// Each workspace has its own working-copy commit. When you have more than one
/// workspace attached to a repo, they are indicated by `@<workspace name>` in
/// `jj log`.
///
/// Each workspace also has own sparse patterns.
#[derive(Subcommand, Clone, Debug)]
pub(crate) enum WorkspaceCommand {
    Add(WorkspaceAddArgs),
    Forget(WorkspaceForgetArgs),
    List(WorkspaceListArgs),
    Root(WorkspaceRootArgs),
    UpdateStale(WorkspaceUpdateStaleArgs),
}

/// Add a workspace
///
/// Sparse patterns will be copied over from the current workspace.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct WorkspaceAddArgs {
    /// Where to create the new workspace
    destination: String,
    /// A name for the workspace
    ///
    /// To override the default, which is the basename of the destination
    /// directory.
    #[arg(long)]
    name: Option<String>,
    /// A list of parent revisions for the working-copy commit of the newly
    /// created workspace. You may specify nothing, or any number of parents.
    ///
    /// If no revisions are specified, the new workspace will be created, and
    /// its working-copy commit will exist on top of the parent(s) of the
    /// working-copy commit in the current workspace, i.e. they will share the
    /// same parent(s).
    ///
    /// If any revisions are specified, the new workspace will be created, and
    /// the new working-copy commit will be created with all these revisions as
    /// parents, i.e. the working-copy commit will exist as if you had run `jj
    /// new r1 r2 r3 ...`.
    #[arg(long, short)]
    revision: Vec<RevisionArg>,
}

/// Stop tracking a workspace's working-copy commit in the repo
///
/// The workspace will not be touched on disk. It can be deleted from disk
/// before or after running this command.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct WorkspaceForgetArgs {
    /// Names of the workspaces to forget. By default, forgets only the current
    /// workspace.
    workspaces: Vec<String>,
}

/// List workspaces
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct WorkspaceListArgs {}

/// Show the current workspace root directory
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct WorkspaceRootArgs {}

/// Update a workspace that has become stale
///
/// For information about stale working copies, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct WorkspaceUpdateStaleArgs {}

#[instrument(skip_all)]
pub(crate) fn cmd_workspace(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &WorkspaceCommand,
) -> Result<(), CommandError> {
    match subcommand {
        WorkspaceCommand::Add(args) => cmd_workspace_add(ui, command, args),
        WorkspaceCommand::Forget(args) => cmd_workspace_forget(ui, command, args),
        WorkspaceCommand::List(args) => cmd_workspace_list(ui, command, args),
        WorkspaceCommand::Root(args) => cmd_workspace_root(ui, command, args),
        WorkspaceCommand::UpdateStale(args) => cmd_workspace_update_stale(ui, command, args),
    }
}

#[instrument(skip_all)]
fn cmd_workspace_add(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &WorkspaceAddArgs,
) -> Result<(), CommandError> {
    let old_workspace_command = command.workspace_helper(ui)?;
    let destination_path = command.cwd().join(&args.destination);
    if destination_path.exists() {
        return Err(user_error("Workspace already exists"));
    } else {
        fs::create_dir(&destination_path).unwrap();
    }
    let name = if let Some(name) = &args.name {
        name.to_string()
    } else {
        destination_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    };
    let workspace_id = WorkspaceId::new(name.clone());
    let repo = old_workspace_command.repo();
    if repo.view().get_wc_commit_id(&workspace_id).is_some() {
        return Err(user_error(format!(
            "Workspace named '{name}' already exists"
        )));
    }

    let working_copy_factory = command.get_working_copy_factory()?;
    let (new_workspace, repo) = Workspace::init_workspace_with_existing_repo(
        command.settings(),
        &destination_path,
        repo,
        working_copy_factory,
        workspace_id,
    )?;
    writeln!(
        ui.status(),
        "Created workspace in \"{}\"",
        file_util::relative_path(old_workspace_command.workspace_root(), &destination_path)
            .display()
    )?;

    // Copy sparse patterns from workspace where the command was run
    let mut new_workspace_command = WorkspaceCommandHelper::new(ui, command, new_workspace, repo)?;
    let (mut locked_ws, _wc_commit) = new_workspace_command.start_working_copy_mutation()?;
    let sparse_patterns = old_workspace_command
        .working_copy()
        .sparse_patterns()?
        .to_vec();
    locked_ws
        .locked_wc()
        .set_sparse_patterns(sparse_patterns)
        .map_err(|err| internal_error_with_message("Failed to set sparse patterns", err))?;
    let operation_id = locked_ws.locked_wc().old_operation_id().clone();
    locked_ws.finish(operation_id)?;

    let mut tx = new_workspace_command.start_transaction();

    // If no parent revisions are specified, create a working-copy commit based
    // on the parent of the current working-copy commit.
    let parents = if args.revision.is_empty() {
        // Check out parents of the current workspace's working-copy commit, or the
        // root if there is no working-copy commit in the current workspace.
        if let Some(old_wc_commit_id) = tx
            .base_repo()
            .view()
            .get_wc_commit_id(old_workspace_command.workspace_id())
        {
            tx.repo().store().get_commit(old_wc_commit_id)?.parents()
        } else {
            vec![tx.repo().store().root_commit()]
        }
    } else {
        resolve_multiple_nonempty_revsets_default_single(&old_workspace_command, &args.revision)?
            .into_iter()
            .collect_vec()
    };

    let tree = merge_commit_trees(tx.repo(), &parents)?;
    let parent_ids = parents.iter().map(|c| c.id().clone()).collect_vec();
    let new_wc_commit = tx
        .mut_repo()
        .new_commit(command.settings(), parent_ids, tree.id())
        .write()?;

    tx.edit(&new_wc_commit)?;
    tx.finish(
        ui,
        format!("Create initial working-copy commit in workspace {}", &name),
    )?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_workspace_forget(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &WorkspaceForgetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let wss: Vec<WorkspaceId> = if args.workspaces.is_empty() {
        vec![workspace_command.workspace_id().clone()]
    } else {
        args.workspaces
            .iter()
            .map(|ws| WorkspaceId::new(ws.to_string()))
            .collect()
    };

    for ws in &wss {
        if workspace_command
            .repo()
            .view()
            .get_wc_commit_id(ws)
            .is_none()
        {
            return Err(user_error(format!("No such workspace: {}", ws.as_str())));
        }
    }

    // bundle every workspace forget into a single transaction, so that e.g.
    // undo correctly restores all of them at once.
    let mut tx = workspace_command.start_transaction();
    wss.iter().for_each(|ws| tx.mut_repo().remove_wc_commit(ws));
    let description = if let [ws] = wss.as_slice() {
        format!("forget workspace {}", ws.as_str())
    } else {
        format!(
            "forget workspaces {}",
            wss.iter().map(|ws| ws.as_str()).join(", ")
        )
    };

    tx.finish(ui, description)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_workspace_list(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let mut formatter = ui.stdout_formatter();
    let template = workspace_command.commit_summary_template();
    for (workspace_id, wc_commit_id) in repo.view().wc_commit_ids().iter().sorted() {
        write!(formatter, "{}: ", workspace_id.as_str())?;
        let commit = repo.store().get_commit(wc_commit_id)?;
        template.format(&commit, formatter.as_mut())?;
        writeln!(formatter)?;
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_workspace_root(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceRootArgs,
) -> Result<(), CommandError> {
    let root = command
        .workspace_loader()?
        .workspace_root()
        .to_str()
        .ok_or_else(|| user_error("The workspace root is not valid UTF-8"))?;
    writeln!(ui.stdout(), "{root}")?;
    Ok(())
}

fn create_and_check_out_recovery_commit(
    ui: &mut Ui,
    command: &CommandHelper,
) -> Result<Arc<ReadonlyRepo>, CommandError> {
    let mut workspace_command = command.workspace_helper_no_snapshot(ui)?;
    let workspace_id = workspace_command.workspace_id().clone();
    let mut tx = workspace_command.start_transaction().into_inner();

    let (mut locked_workspace, commit) =
        workspace_command.unchecked_start_working_copy_mutation()?;
    let commit_id = commit.id();

    let mut_repo = tx.mut_repo();
    let new_commit = mut_repo
        .new_commit(
            command.settings(),
            vec![commit_id.clone()],
            commit.tree_id().clone(),
        )
        .write()?;
    mut_repo.set_wc_commit(workspace_id, new_commit.id().clone())?;
    let repo = tx.commit("recovery commit");

    locked_workspace.locked_wc().recover(&new_commit)?;
    locked_workspace.finish(repo.op_id().clone())?;

    writeln!(
        ui.status(),
        "Created and checked out recovery commit {}",
        short_commit_hash(new_commit.id())
    )?;

    Ok(repo)
}

/// Loads workspace that will diverge from the last working-copy operation.
fn for_stale_working_copy(
    ui: &mut Ui,
    command: &CommandHelper,
) -> Result<(WorkspaceCommandHelper, bool), CommandError> {
    let workspace = command.load_workspace()?;
    let op_store = workspace.repo_loader().op_store();
    let (repo, recovered) = {
        let op_id = workspace.working_copy().operation_id();
        match op_store.read_operation(op_id) {
            Ok(op_data) => (
                workspace.repo_loader().load_at(&Operation::new(
                    op_store.clone(),
                    op_id.clone(),
                    op_data,
                ))?,
                false,
            ),
            Err(e @ OpStoreError::ObjectNotFound { .. }) => {
                writeln!(
                    ui.status(),
                    "Failed to read working copy's current operation; attempting recovery. Error \
                     message from read attempt: {e}"
                )?;
                (create_and_check_out_recovery_commit(ui, command)?, true)
            }
            Err(e) => return Err(e.into()),
        }
    };
    Ok((command.for_loaded_repo(ui, workspace, repo)?, recovered))
}

#[instrument(skip_all)]
fn cmd_workspace_update_stale(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceUpdateStaleArgs,
) -> Result<(), CommandError> {
    // Snapshot the current working copy on top of the last known working-copy
    // operation, then merge the concurrent operations. The wc_commit_id of the
    // merged repo wouldn't change because the old one wins, but it's probably
    // fine if we picked the new wc_commit_id.
    let known_wc_commit = {
        let (mut workspace_command, recovered) = for_stale_working_copy(ui, command)?;
        workspace_command.maybe_snapshot(ui)?;

        if recovered {
            // We have already recovered from the situation that prompted the user to run
            // this command, and it is known that the workspace is not stale
            // (since we just updated it), so we can return early.
            return Ok(());
        }

        let wc_commit_id = workspace_command.get_wc_commit_id().unwrap();
        workspace_command.repo().store().get_commit(wc_commit_id)?
    };
    let mut workspace_command = command.workspace_helper_no_snapshot(ui)?;

    let repo = workspace_command.repo().clone();
    let (mut locked_ws, desired_wc_commit) =
        workspace_command.unchecked_start_working_copy_mutation()?;
    match check_stale_working_copy(locked_ws.locked_wc(), &desired_wc_commit, &repo)? {
        WorkingCopyFreshness::Fresh | WorkingCopyFreshness::Updated(_) => {
            writeln!(
                ui.status(),
                "Nothing to do (the working copy is not stale)."
            )?;
        }
        WorkingCopyFreshness::WorkingCopyStale | WorkingCopyFreshness::SiblingOperation => {
            // The same check as start_working_copy_mutation(), but with the stale
            // working-copy commit.
            if known_wc_commit.tree_id() != locked_ws.locked_wc().old_tree_id() {
                return Err(user_error("Concurrent working copy operation. Try again."));
            }
            let stats = locked_ws
                .locked_wc()
                .check_out(&desired_wc_commit)
                .map_err(|err| {
                    internal_error_with_message(
                        format!(
                            "Failed to check out commit {}",
                            desired_wc_commit.id().hex()
                        ),
                        err,
                    )
                })?;
            locked_ws.finish(repo.op_id().clone())?;
            write!(ui.status(), "Working copy now at: ")?;
            ui.status().with_label("working_copy", |fmt| {
                workspace_command.write_commit_summary(fmt, &desired_wc_commit)
            })?;
            writeln!(ui.status())?;
            print_checkout_stats(ui, stats, &desired_wc_commit)?;
        }
    }
    Ok(())
}
