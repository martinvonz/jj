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

mod abandon;
mod backout;
#[cfg(feature = "bench")]
mod bench;
mod branch;
mod cat;
mod checkout;
mod chmod;
mod commit;
mod config;
mod debug;
mod describe;
mod diff;
mod diffedit;
mod duplicate;
mod edit;
mod files;
mod git;
mod init;
mod interdiff;
mod log;
mod merge;
mod r#move;
mod new;
mod next;
mod obslog;
mod operation;
mod prev;
mod rebase;
mod resolve;
mod restore;
mod run;
mod show;
mod sparse;
mod split;
mod squash;
mod status;
mod util;

use std::fmt::Debug;
use std::io::Write;
use std::{fmt, fs, io};

use clap::{Command, CommandFactory, FromArgMatches, Subcommand};
use itertools::Itertools;
use jj_lib::backend::ObjectId;
use jj_lib::commit::Commit;
use jj_lib::file_util;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::merge::Merge;
use jj_lib::merged_tree::{MergedTree, MergedTreeBuilder};
use jj_lib::op_store::WorkspaceId;
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::rewrite::merge_commit_trees;
use jj_lib::settings::UserSettings;
use jj_lib::working_copy::SnapshotOptions;
use jj_lib::workspace::{default_working_copy_initializer, Workspace};
use tracing::instrument;

use crate::cli_util::{
    check_stale_working_copy, print_checkout_stats, run_ui_editor, user_error,
    user_error_with_hint, Args, CommandError, CommandHelper, RevisionArg, WorkspaceCommandHelper,
};
use crate::diff_util::{self, DiffFormat};
use crate::formatter::{Formatter, PlainTextFormatter};
use crate::text_util;
use crate::ui::Ui;

#[derive(clap::Parser, Clone, Debug)]
enum Commands {
    Abandon(abandon::AbandonArgs),
    Backout(backout::BackoutArgs),
    #[cfg(feature = "bench")]
    #[command(subcommand)]
    Bench(bench::BenchCommands),
    #[command(subcommand)]
    Branch(branch::BranchSubcommand),
    #[command(alias = "print")]
    Cat(cat::CatArgs),
    Checkout(checkout::CheckoutArgs),
    Chmod(chmod::ChmodArgs),
    Commit(commit::CommitArgs),
    #[command(subcommand)]
    Config(config::ConfigSubcommand),
    #[command(subcommand)]
    Debug(debug::DebugCommands),
    Describe(describe::DescribeArgs),
    Diff(diff::DiffArgs),
    Diffedit(diffedit::DiffeditArgs),
    Duplicate(duplicate::DuplicateArgs),
    Edit(edit::EditArgs),
    Files(files::FilesArgs),
    #[command(subcommand)]
    Git(git::GitCommands),
    Init(init::InitArgs),
    Interdiff(interdiff::InterdiffArgs),
    Log(log::LogArgs),
    /// Merge work from multiple branches
    ///
    /// Unlike most other VCSs, `jj merge` does not implicitly include the
    /// working copy revision's parent as one of the parents of the merge;
    /// you need to explicitly list all revisions that should become parents
    /// of the merge.
    ///
    /// This is the same as `jj new`, except that it requires at least two
    /// arguments.
    Merge(new::NewArgs),
    Move(r#move::MoveArgs),
    New(new::NewArgs),
    Next(next::NextArgs),
    Obslog(obslog::ObslogArgs),
    #[command(subcommand)]
    #[command(visible_alias = "op")]
    Operation(operation::OperationCommands),
    Prev(prev::PrevArgs),
    Rebase(rebase::RebaseArgs),
    Resolve(resolve::ResolveArgs),
    Restore(restore::RestoreArgs),
    #[command(hide = true)]
    // TODO: Flesh out.
    Run(run::RunArgs),
    Show(show::ShowArgs),
    #[command(subcommand)]
    Sparse(sparse::SparseArgs),
    Split(split::SplitArgs),
    Squash(squash::SquashArgs),
    Status(status::StatusArgs),
    #[command(subcommand)]
    Util(util::UtilCommands),
    /// Undo an operation (shortcut for `jj op undo`)
    Undo(operation::OperationUndoArgs),
    Unsquash(UnsquashArgs),
    Untrack(UntrackArgs),
    Version(VersionArgs),
    #[command(subcommand)]
    Workspace(WorkspaceCommands),
}

/// Display version information
#[derive(clap::Args, Clone, Debug)]
struct VersionArgs {}

/// Stop tracking specified paths in the working copy
#[derive(clap::Args, Clone, Debug)]
struct UntrackArgs {
    /// Paths to untrack
    #[arg(required = true, value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

/// Move changes from a revision's parent into the revision
///
/// After moving the changes out of the parent, the child revision will have the
/// same content state as before. If moving the change out of the parent change
/// made it empty compared to its parent, it will be abandoned. Without
/// `--interactive`, the parent change will always become empty.
///
/// If the source became empty and both the source and destination had a
/// non-empty description, you will be asked for the combined description. If
/// either was empty, then the other one will be used.
#[derive(clap::Args, Clone, Debug)]
#[command(visible_alias = "unamend")]
struct UnsquashArgs {
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Interactively choose which parts to unsquash
    // TODO: It doesn't make much sense to run this without -i. We should make that
    // the default.
    #[arg(long, short)]
    interactive: bool,
}

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
enum WorkspaceCommands {
    Add(WorkspaceAddArgs),
    Forget(WorkspaceForgetArgs),
    List(WorkspaceListArgs),
    Root(WorkspaceRootArgs),
    UpdateStale(WorkspaceUpdateStaleArgs),
}

/// Add a workspace
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceAddArgs {
    /// Where to create the new workspace
    destination: String,
    /// A name for the workspace
    ///
    /// To override the default, which is the basename of the destination
    /// directory.
    #[arg(long)]
    name: Option<String>,
    /// The revision that the workspace should be created at; a new working copy
    /// commit will be created on top of it.
    #[arg(long, short)]
    revision: Option<RevisionArg>,
}

/// Stop tracking a workspace's working-copy commit in the repo
///
/// The workspace will not be touched on disk. It can be deleted from disk
/// before or after running this command.
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceForgetArgs {
    /// Names of the workspaces to forget. By default, forgets only the current
    /// workspace.
    workspaces: Vec<String>,
}

/// List workspaces
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceListArgs {}

/// Show the current workspace root directory
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceRootArgs {}

/// Update a workspace that has become stale
///
/// For information about stale working copies, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceUpdateStaleArgs {}

#[instrument(skip_all)]
fn cmd_version(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &VersionArgs,
) -> Result<(), CommandError> {
    write!(ui.stdout(), "{}", command.app().render_version())?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_untrack(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &UntrackArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let store = workspace_command.repo().store().clone();
    let matcher = workspace_command.matcher_from_values(&args.paths)?;

    let mut tx = workspace_command
        .start_transaction("untrack paths")
        .into_inner();
    let base_ignores = workspace_command.base_ignores();
    let (mut locked_ws, wc_commit) = workspace_command.start_working_copy_mutation()?;
    // Create a new tree without the unwanted files
    let mut tree_builder = MergedTreeBuilder::new(wc_commit.tree_id().clone());
    let wc_tree = wc_commit.tree()?;
    for (path, _value) in wc_tree.entries_matching(matcher.as_ref()) {
        tree_builder.set_or_remove(path, Merge::absent());
    }
    let new_tree_id = tree_builder.write_tree(&store)?;
    let new_tree = store.get_root_tree(&new_tree_id)?;
    // Reset the working copy to the new tree
    locked_ws.locked_wc().reset(&new_tree)?;
    // Commit the working copy again so we can inform the user if paths couldn't be
    // untracked because they're not ignored.
    let wc_tree_id = locked_ws.locked_wc().snapshot(SnapshotOptions {
        base_ignores,
        fsmonitor_kind: command.settings().fsmonitor_kind()?,
        progress: None,
        max_new_file_size: command.settings().max_new_file_size()?,
    })?;
    if wc_tree_id != new_tree_id {
        let wc_tree = store.get_root_tree(&wc_tree_id)?;
        let added_back = wc_tree.entries_matching(matcher.as_ref()).collect_vec();
        if !added_back.is_empty() {
            drop(locked_ws);
            let path = &added_back[0].0;
            let ui_path = workspace_command.format_file_path(path);
            let message = if added_back.len() > 1 {
                format!(
                    "'{}' and {} other files are not ignored.",
                    ui_path,
                    added_back.len() - 1
                )
            } else {
                format!("'{ui_path}' is not ignored.")
            };
            return Err(user_error_with_hint(
                message,
                "Files that are not ignored will be added back by the next command.
Make sure they're ignored, then try again.",
            ));
        } else {
            // This means there were some concurrent changes made in the working copy. We
            // don't want to mix those in, so reset the working copy again.
            locked_ws.locked_wc().reset(&new_tree)?;
        }
    }
    tx.mut_repo()
        .rewrite_commit(command.settings(), &wc_commit)
        .set_tree_id(new_tree_id)
        .write()?;
    let num_rebased = tx.mut_repo().rebase_descendants(command.settings())?;
    if num_rebased > 0 {
        writeln!(ui.stderr(), "Rebased {num_rebased} descendant commits")?;
    }
    let repo = tx.commit();
    locked_ws.finish(repo.op_id().clone())?;
    Ok(())
}

fn show_predecessor_patch(
    ui: &Ui,
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
    diff_formats: &[DiffFormat],
) -> Result<(), CommandError> {
    let predecessors = commit.predecessors();
    let predecessor = match predecessors.first() {
        Some(predecessor) => predecessor,
        None => return Ok(()),
    };
    let predecessor_tree = rebase_to_dest_parent(workspace_command, predecessor, commit)?;
    let tree = commit.tree()?;
    diff_util::show_diff(
        ui,
        formatter,
        workspace_command,
        &predecessor_tree,
        &tree,
        &EverythingMatcher,
        diff_formats,
    )
}

fn rebase_to_dest_parent(
    workspace_command: &WorkspaceCommandHelper,
    source: &Commit,
    destination: &Commit,
) -> Result<MergedTree, CommandError> {
    if source.parent_ids() == destination.parent_ids() {
        Ok(source.tree()?)
    } else {
        let destination_parent_tree =
            merge_commit_trees(workspace_command.repo().as_ref(), &destination.parents())?;
        let source_parent_tree =
            merge_commit_trees(workspace_command.repo().as_ref(), &source.parents())?;
        let source_tree = source.tree()?;
        let rebased_tree = destination_parent_tree.merge(&source_parent_tree, &source_tree)?;
        Ok(rebased_tree)
    }
}

fn edit_description(
    repo: &ReadonlyRepo,
    description: &str,
    settings: &UserSettings,
) -> Result<String, CommandError> {
    let description_file_path = (|| -> Result<_, io::Error> {
        let mut file = tempfile::Builder::new()
            .prefix("editor-")
            .suffix(".jjdescription")
            .tempfile_in(repo.repo_path())?;
        file.write_all(description.as_bytes())?;
        file.write_all(b"\nJJ: Lines starting with \"JJ: \" (like this one) will be removed.\n")?;
        let (_, path) = file.keep().map_err(|e| e.error)?;
        Ok(path)
    })()
    .map_err(|e| {
        user_error(format!(
            r#"Failed to create description file in "{path}": {e}"#,
            path = repo.repo_path().display()
        ))
    })?;

    run_ui_editor(settings, &description_file_path)?;

    let description = fs::read_to_string(&description_file_path).map_err(|e| {
        user_error(format!(
            r#"Failed to read description file "{path}": {e}"#,
            path = description_file_path.display()
        ))
    })?;
    // Delete the file only if everything went well.
    // TODO: Tell the user the name of the file we left behind.
    std::fs::remove_file(description_file_path).ok();
    // Normalize line ending, remove leading and trailing blank lines.
    let description = description
        .lines()
        .filter(|line| !line.starts_with("JJ: "))
        .join("\n");
    Ok(text_util::complete_newline(description.trim_matches('\n')))
}

fn combine_messages(
    repo: &ReadonlyRepo,
    source: &Commit,
    destination: &Commit,
    settings: &UserSettings,
    abandon_source: bool,
) -> Result<String, CommandError> {
    let description = if abandon_source {
        if source.description().is_empty() {
            destination.description().to_string()
        } else if destination.description().is_empty() {
            source.description().to_string()
        } else {
            let combined = "JJ: Enter a description for the combined commit.\n".to_string()
                + "JJ: Description from the destination commit:\n"
                + destination.description()
                + "\nJJ: Description from the source commit:\n"
                + source.description();
            edit_description(repo, &combined, settings)?
        }
    } else {
        destination.description().to_string()
    };
    Ok(description)
}

#[instrument(skip_all)]
fn cmd_unsquash(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &UnsquashArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    workspace_command.check_rewritable([&commit])?;
    let parents = commit.parents();
    if parents.len() != 1 {
        return Err(user_error("Cannot unsquash merge commits"));
    }
    let parent = &parents[0];
    workspace_command.check_rewritable(&parents[..1])?;
    let mut tx =
        workspace_command.start_transaction(&format!("unsquash commit {}", commit.id().hex()));
    let parent_base_tree = merge_commit_trees(tx.repo(), &parent.parents())?;
    let new_parent_tree_id;
    if args.interactive {
        let instructions = format!(
            "\
You are moving changes from: {}
into its child: {}

The diff initially shows the parent commit's changes.

Adjust the right side until it shows the contents you want to keep in
the parent commit. The changes you edited out will be moved into the
child commit. If you don't make any changes, then the operation will be
aborted.
",
            tx.format_commit_summary(parent),
            tx.format_commit_summary(&commit)
        );
        let parent_tree = parent.tree()?;
        new_parent_tree_id = tx.edit_diff(
            ui,
            &parent_base_tree,
            &parent_tree,
            &EverythingMatcher,
            &instructions,
        )?;
        if new_parent_tree_id == parent_base_tree.id() {
            return Err(user_error("No changes selected"));
        }
    } else {
        new_parent_tree_id = parent_base_tree.id().clone();
    }
    // Abandon the parent if it is now empty (always the case in the non-interactive
    // case).
    if new_parent_tree_id == parent_base_tree.id() {
        tx.mut_repo().record_abandoned_commit(parent.id().clone());
        let description =
            combine_messages(tx.base_repo(), parent, &commit, command.settings(), true)?;
        // Commit the new child on top of the parent's parents.
        tx.mut_repo()
            .rewrite_commit(command.settings(), &commit)
            .set_parents(parent.parent_ids().to_vec())
            .set_description(description)
            .write()?;
    } else {
        let new_parent = tx
            .mut_repo()
            .rewrite_commit(command.settings(), parent)
            .set_tree_id(new_parent_tree_id)
            .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
            .write()?;
        // Commit the new child on top of the new parent.
        tx.mut_repo()
            .rewrite_commit(command.settings(), &commit)
            .set_parents(vec![new_parent.id().clone()])
            .write()?;
    }
    tx.finish(ui)?;
    Ok(())
}

fn description_template_for_commit(
    ui: &Ui,
    settings: &UserSettings,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
) -> Result<String, CommandError> {
    let mut diff_summary_bytes = Vec::new();
    diff_util::show_patch(
        ui,
        &mut PlainTextFormatter::new(&mut diff_summary_bytes),
        workspace_command,
        commit,
        &EverythingMatcher,
        &[DiffFormat::Summary],
    )?;
    let description = if commit.description().is_empty() {
        settings.default_description()
    } else {
        commit.description().to_owned()
    };
    if diff_summary_bytes.is_empty() {
        Ok(description)
    } else {
        Ok(description + "\n" + &diff_summary_to_description(&diff_summary_bytes))
    }
}

fn diff_summary_to_description(bytes: &[u8]) -> String {
    let text = std::str::from_utf8(bytes).expect(
        "Summary diffs and repo paths must always be valid UTF8.",
        // Double-check this assumption for diffs that include file content.
    );
    "JJ: This commit contains the following changes:\n".to_owned()
        + &textwrap::indent(text, "JJ:     ")
}

fn make_branch_term(branch_names: &[impl fmt::Display]) -> String {
    match branch_names {
        [branch_name] => format!("branch {}", branch_name),
        branch_names => format!("branches {}", branch_names.iter().join(", ")),
    }
}

#[instrument(skip_all)]
fn cmd_workspace(
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

    let parents = if let Some(specific_rev) = &args.revision {
        vec![old_workspace_command.resolve_single_rev(specific_rev, ui)?]
    } else {
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

pub fn default_app() -> Command {
    Commands::augment_subcommands(Args::command())
}

#[instrument(skip_all)]
pub fn run_command(ui: &mut Ui, command_helper: &CommandHelper) -> Result<(), CommandError> {
    let derived_subcommands: Commands =
        Commands::from_arg_matches(command_helper.matches()).unwrap();
    match &derived_subcommands {
        Commands::Version(sub_args) => cmd_version(ui, command_helper, sub_args),
        Commands::Init(sub_args) => init::cmd_init(ui, command_helper, sub_args),
        Commands::Config(sub_args) => config::cmd_config(ui, command_helper, sub_args),
        Commands::Checkout(sub_args) => checkout::cmd_checkout(ui, command_helper, sub_args),
        Commands::Untrack(sub_args) => cmd_untrack(ui, command_helper, sub_args),
        Commands::Files(sub_args) => files::cmd_files(ui, command_helper, sub_args),
        Commands::Cat(sub_args) => cat::cmd_cat(ui, command_helper, sub_args),
        Commands::Diff(sub_args) => diff::cmd_diff(ui, command_helper, sub_args),
        Commands::Show(sub_args) => show::cmd_show(ui, command_helper, sub_args),
        Commands::Status(sub_args) => status::cmd_status(ui, command_helper, sub_args),
        Commands::Log(sub_args) => log::cmd_log(ui, command_helper, sub_args),
        Commands::Interdiff(sub_args) => interdiff::cmd_interdiff(ui, command_helper, sub_args),
        Commands::Obslog(sub_args) => obslog::cmd_obslog(ui, command_helper, sub_args),
        Commands::Describe(sub_args) => describe::cmd_describe(ui, command_helper, sub_args),
        Commands::Commit(sub_args) => commit::cmd_commit(ui, command_helper, sub_args),
        Commands::Duplicate(sub_args) => duplicate::cmd_duplicate(ui, command_helper, sub_args),
        Commands::Abandon(sub_args) => abandon::cmd_abandon(ui, command_helper, sub_args),
        Commands::Edit(sub_args) => edit::cmd_edit(ui, command_helper, sub_args),
        Commands::Next(sub_args) => next::cmd_next(ui, command_helper, sub_args),
        Commands::Prev(sub_args) => prev::cmd_prev(ui, command_helper, sub_args),
        Commands::New(sub_args) => new::cmd_new(ui, command_helper, sub_args),
        Commands::Move(sub_args) => r#move::cmd_move(ui, command_helper, sub_args),
        Commands::Squash(sub_args) => squash::cmd_squash(ui, command_helper, sub_args),
        Commands::Unsquash(sub_args) => cmd_unsquash(ui, command_helper, sub_args),
        Commands::Restore(sub_args) => restore::cmd_restore(ui, command_helper, sub_args),
        Commands::Run(sub_args) => run::cmd_run(ui, command_helper, sub_args),
        Commands::Diffedit(sub_args) => diffedit::cmd_diffedit(ui, command_helper, sub_args),
        Commands::Split(sub_args) => split::cmd_split(ui, command_helper, sub_args),
        Commands::Merge(sub_args) => merge::cmd_merge(ui, command_helper, sub_args),
        Commands::Rebase(sub_args) => rebase::cmd_rebase(ui, command_helper, sub_args),
        Commands::Backout(sub_args) => backout::cmd_backout(ui, command_helper, sub_args),
        Commands::Resolve(sub_args) => resolve::cmd_resolve(ui, command_helper, sub_args),
        Commands::Branch(sub_args) => branch::cmd_branch(ui, command_helper, sub_args),
        Commands::Undo(sub_args) => operation::cmd_op_undo(ui, command_helper, sub_args),
        Commands::Operation(sub_args) => operation::cmd_operation(ui, command_helper, sub_args),
        Commands::Workspace(sub_args) => cmd_workspace(ui, command_helper, sub_args),
        Commands::Sparse(sub_args) => sparse::cmd_sparse(ui, command_helper, sub_args),
        Commands::Chmod(sub_args) => chmod::cmd_chmod(ui, command_helper, sub_args),
        Commands::Git(sub_args) => git::cmd_git(ui, command_helper, sub_args),
        Commands::Util(sub_args) => util::cmd_util(ui, command_helper, sub_args),
        #[cfg(feature = "bench")]
        Commands::Bench(sub_args) => bench::cmd_bench(ui, command_helper, sub_args),
        Commands::Debug(sub_args) => debug::cmd_debug(ui, command_helper, sub_args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_app() {
        default_app().debug_assert();
    }
}
