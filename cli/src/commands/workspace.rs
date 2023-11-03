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

use clap::Subcommand;
use itertools::Itertools;
use jj_lib::backend::ObjectId;
use jj_lib::file_util;
use jj_lib::op_store::WorkspaceId;
use jj_lib::repo::Repo;
use jj_lib::rewrite::merge_commit_trees;
use jj_lib::workspace::{default_working_copy_initializer, Workspace};
use tracing::instrument;

use crate::cli_util::{
    check_stale_working_copy, print_checkout_stats, user_error, CommandError, CommandHelper,
    RevisionArg, WorkspaceCommandHelper,
};
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
#[derive(Subcommand, Clone, Debug)]
pub(crate) enum WorkspaceCommands {
    Add(WorkspaceAddArgs),
    Forget(WorkspaceForgetArgs),
    List(WorkspaceListArgs),
    Root(WorkspaceRootArgs),
    UpdateStale(WorkspaceUpdateStaleArgs),
}

/// Add a workspace
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
    subcommand: &WorkspaceCommands,
) -> Result<(), CommandError> {
    match subcommand {
        WorkspaceCommands::Add(command_matches) => cmd_workspace_add(ui, command, command_matches),
        WorkspaceCommands::Forget(command_matches) => {
            cmd_workspace_forget(ui, command, command_matches)
        }
        WorkspaceCommands::List(command_matches) => {
            cmd_workspace_list(ui, command, command_matches)
        }
        WorkspaceCommands::Root(command_matches) => {
            cmd_workspace_root(ui, command, command_matches)
        }
        WorkspaceCommands::UpdateStale(command_matches) => {
            cmd_workspace_update_stale(ui, command, command_matches)
        }
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
    // TODO: How do we create a workspace with a non-default working copy?
    let (new_workspace, repo) = Workspace::init_workspace_with_existing_repo(
        command.settings(),
        &destination_path,
        repo,
        default_working_copy_initializer(),
        workspace_id,
    )?;
    writeln!(
        ui.stderr(),
        "Created workspace in \"{}\"",
        file_util::relative_path(old_workspace_command.workspace_root(), &destination_path)
            .display()
    )?;

    let mut new_workspace_command = WorkspaceCommandHelper::new(ui, command, new_workspace, repo)?;
    let mut tx = new_workspace_command.start_transaction(&format!(
        "Create initial working-copy commit in workspace {}",
        &name
    ));

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
        crate::commands::rebase::resolve_destination_revs(
            &old_workspace_command,
            ui,
            &args.revision,
        )?
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
    tx.finish(ui)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_workspace_forget(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &WorkspaceForgetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let len = args.workspaces.len();

    let mut wss = Vec::new();
    let description = match len {
        // NOTE (aseipp): if there's only 1-or-0 arguments, shortcut. this is
        // mostly so the oplog description can look good: it removes the need,
        // in the case of more-than-1 argument, to handle pluralization of the
        // nouns in the description
        0 | 1 => {
            let ws = match len == 0 {
                true => workspace_command.workspace_id().to_owned(),
                false => WorkspaceId::new(args.workspaces[0].to_string()),
            };
            wss.push(ws.clone());
            format!("forget workspace {}", ws.as_str())
        }
        _ => {
            args.workspaces
                .iter()
                .map(|ws| WorkspaceId::new(ws.to_string()))
                .for_each(|ws| wss.push(ws));

            format!("forget workspaces {}", args.workspaces.join(", "))
        }
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
    let mut tx = workspace_command.start_transaction(&description);
    wss.iter().for_each(|ws| tx.mut_repo().remove_wc_commit(ws));
    tx.finish(ui)?;
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
    for (workspace_id, wc_commit_id) in repo.view().wc_commit_ids().iter().sorted() {
        write!(ui.stdout(), "{}: ", workspace_id.as_str())?;
        let commit = repo.store().get_commit(wc_commit_id)?;
        workspace_command.write_commit_summary(ui.stdout_formatter().as_mut(), &commit)?;
        writeln!(ui.stdout())?;
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_workspace_root(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceRootArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let root = workspace_command
        .workspace_root()
        .to_str()
        .ok_or_else(|| user_error("The workspace root is not valid UTF-8"))?;
    writeln!(ui.stdout(), "{root}")?;
    Ok(())
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
        let mut workspace_command = command.for_stale_working_copy(ui)?;
        workspace_command.snapshot(ui)?;
        let wc_commit_id = workspace_command.get_wc_commit_id().unwrap();
        workspace_command.repo().store().get_commit(wc_commit_id)?
    };
    let mut workspace_command = command.workspace_helper_no_snapshot(ui)?;

    let repo = workspace_command.repo().clone();
    let (mut locked_ws, desired_wc_commit) =
        workspace_command.unchecked_start_working_copy_mutation()?;
    match check_stale_working_copy(locked_ws.locked_wc(), &desired_wc_commit, &repo) {
        Ok(_) => {
            writeln!(
                ui.stderr(),
                "Nothing to do (the working copy is not stale)."
            )?;
        }
        Err(_) => {
            // The same check as start_working_copy_mutation(), but with the stale
            // working-copy commit.
            if known_wc_commit.tree_id() != locked_ws.locked_wc().old_tree_id() {
                return Err(user_error("Concurrent working copy operation. Try again."));
            }
            let stats = locked_ws
                .locked_wc()
                .check_out(&desired_wc_commit)
                .map_err(|err| {
                    CommandError::InternalError(format!(
                        "Failed to check out commit {}: {}",
                        desired_wc_commit.id().hex(),
                        err
                    ))
                })?;
            locked_ws.finish(repo.op_id().clone())?;
            write!(ui.stderr(), "Working copy now at: ")?;
            ui.stderr_formatter().with_label("working_copy", |fmt| {
                workspace_command.write_commit_summary(fmt, &desired_wc_commit)
            })?;
            writeln!(ui.stderr())?;
            print_checkout_stats(ui, stats, &desired_wc_commit)?;
        }
    }
    Ok(())
}
