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
use std::fmt::Debug;
use std::io::Write as _;

use clap::Subcommand;
use jj_lib::backend::TreeId;
use jj_lib::default_index::{AsCompositeIndex as _, DefaultIndexStore, DefaultReadonlyIndex};
use jj_lib::local_working_copy::LocalWorkingCopy;
use jj_lib::merged_tree::MergedTree;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::working_copy::WorkingCopy;
use jj_lib::{op_walk, revset};

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::{internal_error, user_error, CommandError};
use crate::ui::Ui;
use crate::{revset_util, template_parser};

/// Low-level commands not intended for users
#[derive(Subcommand, Clone, Debug)]
#[command(hide = true)]
pub enum DebugCommand {
    Revset(DebugRevsetArgs),
    #[command(name = "workingcopy")]
    WorkingCopy(DebugWorkingCopyArgs),
    Template(DebugTemplateArgs),
    Index(DebugIndexArgs),
    #[command(name = "reindex")]
    ReIndex(DebugReIndexArgs),
    #[command(visible_alias = "view")]
    Operation(DebugOperationArgs),
    Tree(DebugTreeArgs),
    #[command(subcommand)]
    Watchman(DebugWatchmanSubcommand),
}

/// Evaluate revset to full commit IDs
#[derive(clap::Args, Clone, Debug)]
pub struct DebugRevsetArgs {
    revision: String,
}

/// Show information about the working copy state
#[derive(clap::Args, Clone, Debug)]
pub struct DebugWorkingCopyArgs {}

/// Parse a template
#[derive(clap::Args, Clone, Debug)]
pub struct DebugTemplateArgs {
    template: String,
}

/// Show commit index stats
#[derive(clap::Args, Clone, Debug)]
pub struct DebugIndexArgs {}

/// Rebuild commit index
#[derive(clap::Args, Clone, Debug)]
pub struct DebugReIndexArgs {}

/// Show information about an operation and its view
#[derive(clap::Args, Clone, Debug)]
pub struct DebugOperationArgs {
    #[arg(default_value = "@")]
    operation: String,
    #[arg(long, value_enum, default_value = "all")]
    display: DebugOperationDisplay,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum DebugOperationDisplay {
    /// Show only the operation details.
    Operation,
    /// Show the operation id only
    Id,
    /// Show only the view details
    View,
    /// Show both the view and the operation
    All,
}

/// List the recursive entries of a tree.
#[derive(clap::Args, Clone, Debug)]
pub struct DebugTreeArgs {
    #[arg(long, short = 'r')]
    revision: Option<RevisionArg>,
    #[arg(long, conflicts_with = "revision")]
    id: Option<String>,
    #[arg(long, requires = "id")]
    dir: Option<String>,
    paths: Vec<String>,
    // TODO: Add an option to include trees that are ancestors of the matched paths
}

#[derive(Subcommand, Clone, Debug)]
pub enum DebugWatchmanSubcommand {
    QueryClock,
    QueryChangedFiles,
    ResetClock,
}

pub fn cmd_debug(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &DebugCommand,
) -> Result<(), CommandError> {
    match subcommand {
        DebugCommand::Revset(args) => cmd_debug_revset(ui, command, args),
        DebugCommand::WorkingCopy(args) => cmd_debug_working_copy(ui, command, args),
        DebugCommand::Template(args) => cmd_debug_template(ui, command, args),
        DebugCommand::Index(args) => cmd_debug_index(ui, command, args),
        DebugCommand::ReIndex(args) => cmd_debug_reindex(ui, command, args),
        DebugCommand::Operation(args) => cmd_debug_operation(ui, command, args),
        DebugCommand::Tree(args) => cmd_debug_tree(ui, command, args),
        DebugCommand::Watchman(args) => cmd_debug_watchman(ui, command, args),
    }
}

fn cmd_debug_revset(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DebugRevsetArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let workspace_ctx = workspace_command.revset_parse_context();
    let repo = workspace_command.repo().as_ref();

    let expression = revset::parse(&args.revision, &workspace_ctx)?;
    writeln!(ui.stdout(), "-- Parsed:")?;
    writeln!(ui.stdout(), "{expression:#?}")?;
    writeln!(ui.stdout())?;

    let expression = revset::optimize(expression);
    writeln!(ui.stdout(), "-- Optimized:")?;
    writeln!(ui.stdout(), "{expression:#?}")?;
    writeln!(ui.stdout())?;

    let symbol_resolver =
        revset_util::default_symbol_resolver(repo, workspace_command.id_prefix_context()?);
    let expression = expression.resolve_user_expression(repo, &symbol_resolver)?;
    writeln!(ui.stdout(), "-- Resolved:")?;
    writeln!(ui.stdout(), "{expression:#?}")?;
    writeln!(ui.stdout())?;

    let revset = expression.evaluate(repo)?;
    writeln!(ui.stdout(), "-- Evaluated:")?;
    writeln!(ui.stdout(), "{revset:#?}")?;
    writeln!(ui.stdout())?;

    writeln!(ui.stdout(), "-- Commit IDs:")?;
    for commit_id in revset.iter() {
        writeln!(ui.stdout(), "{}", commit_id.hex())?;
    }
    Ok(())
}

fn cmd_debug_working_copy(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &DebugWorkingCopyArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let wc = check_local_disk_wc(workspace_command.working_copy().as_any())?;
    writeln!(ui.stdout(), "Current operation: {:?}", wc.operation_id())?;
    writeln!(ui.stdout(), "Current tree: {:?}", wc.tree_id()?)?;
    for (file, state) in wc.file_states()? {
        writeln!(
            ui.stdout(),
            "{:?} {:13?} {:10?} {:?}",
            state.file_type,
            state.size,
            state.mtime.0,
            file
        )?;
    }
    Ok(())
}

fn cmd_debug_template(
    ui: &mut Ui,
    _command: &CommandHelper,
    args: &DebugTemplateArgs,
) -> Result<(), CommandError> {
    let node = template_parser::parse_template(&args.template)?;
    writeln!(ui.stdout(), "{node:#?}")?;
    Ok(())
}

fn cmd_debug_index(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &DebugIndexArgs,
) -> Result<(), CommandError> {
    // Resolve the operation without loading the repo, so this command won't
    // merge concurrent operations and update the index.
    let workspace = command.load_workspace()?;
    let repo_loader = workspace.repo_loader();
    let op = op_walk::resolve_op_for_load(repo_loader, &command.global_args().at_operation)?;
    let index_store = repo_loader.index_store();
    let index = index_store
        .get_index_at_op(&op, repo_loader.store())
        .map_err(internal_error)?;
    if let Some(default_index) = index.as_any().downcast_ref::<DefaultReadonlyIndex>() {
        let stats = default_index.as_composite().stats();
        writeln!(ui.stdout(), "Number of commits: {}", stats.num_commits)?;
        writeln!(ui.stdout(), "Number of merges: {}", stats.num_merges)?;
        writeln!(
            ui.stdout(),
            "Max generation number: {}",
            stats.max_generation_number
        )?;
        writeln!(ui.stdout(), "Number of heads: {}", stats.num_heads)?;
        writeln!(ui.stdout(), "Number of changes: {}", stats.num_changes)?;
        writeln!(ui.stdout(), "Stats per level:")?;
        for (i, level) in stats.levels.iter().enumerate() {
            writeln!(ui.stdout(), "  Level {i}:")?;
            writeln!(ui.stdout(), "    Number of commits: {}", level.num_commits)?;
            writeln!(ui.stdout(), "    Name: {}", level.name.as_ref().unwrap())?;
        }
    } else {
        return Err(user_error(format!(
            "Cannot get stats for indexes of type '{}'",
            index_store.name()
        )));
    }
    Ok(())
}

fn cmd_debug_reindex(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &DebugReIndexArgs,
) -> Result<(), CommandError> {
    // Resolve the operation without loading the repo. The index might have to
    // be rebuilt while loading the repo.
    let workspace = command.load_workspace()?;
    let repo_loader = workspace.repo_loader();
    let op = op_walk::resolve_op_for_load(repo_loader, &command.global_args().at_operation)?;
    let index_store = repo_loader.index_store();
    if let Some(default_index_store) = index_store.as_any().downcast_ref::<DefaultIndexStore>() {
        default_index_store.reinit().map_err(internal_error)?;
        let default_index = default_index_store
            .build_index_at_operation(&op, repo_loader.store())
            .map_err(internal_error)?;
        writeln!(
            ui.stderr(),
            "Finished indexing {:?} commits.",
            default_index.as_composite().stats().num_commits
        )?;
    } else {
        return Err(user_error(format!(
            "Cannot reindex indexes of type '{}'",
            index_store.name()
        )));
    }
    Ok(())
}

fn cmd_debug_operation(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DebugOperationArgs,
) -> Result<(), CommandError> {
    // Resolve the operation without loading the repo, so this command can be used
    // even if e.g. the view object is broken.
    let workspace = command.load_workspace()?;
    let repo_loader = workspace.repo_loader();
    let op = op_walk::resolve_op_for_load(repo_loader, &args.operation)?;
    if args.display == DebugOperationDisplay::Id {
        writeln!(ui.stdout(), "{}", op.id().hex())?;
        return Ok(());
    }
    if args.display != DebugOperationDisplay::View {
        writeln!(ui.stdout(), "{:#?}", op.store_operation())?;
    }
    if args.display != DebugOperationDisplay::Operation {
        writeln!(ui.stdout(), "{:#?}", op.view()?.store_view())?;
    }
    Ok(())
}

fn cmd_debug_tree(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DebugTreeArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let tree = if let Some(tree_id_hex) = &args.id {
        let tree_id =
            TreeId::try_from_hex(tree_id_hex).map_err(|_| user_error("Invalid tree id"))?;
        let dir = if let Some(dir_str) = &args.dir {
            workspace_command.parse_file_path(dir_str)?
        } else {
            RepoPathBuf::root()
        };
        let store = workspace_command.repo().store();
        let tree = store.get_tree(&dir, &tree_id)?;
        MergedTree::resolved(tree)
    } else {
        let commit =
            workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"))?;
        commit.tree()?
    };
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    for (path, value) in tree.entries_matching(matcher.as_ref()) {
        let ui_path = workspace_command.format_file_path(&path);
        writeln!(ui.stdout(), "{ui_path}: {value:?}")?;
    }

    Ok(())
}

#[cfg(feature = "watchman")]
fn cmd_debug_watchman(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &DebugWatchmanSubcommand,
) -> Result<(), CommandError> {
    use jj_lib::local_working_copy::LockedLocalWorkingCopy;

    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo().clone();
    match subcommand {
        DebugWatchmanSubcommand::QueryClock => {
            let wc = check_local_disk_wc(workspace_command.working_copy().as_any())?;
            let (clock, _changed_files) = wc.query_watchman()?;
            writeln!(ui.stdout(), "Clock: {clock:?}")?;
        }
        DebugWatchmanSubcommand::QueryChangedFiles => {
            let wc = check_local_disk_wc(workspace_command.working_copy().as_any())?;
            let (_clock, changed_files) = wc.query_watchman()?;
            writeln!(ui.stdout(), "Changed files: {changed_files:?}")?;
        }
        DebugWatchmanSubcommand::ResetClock => {
            let (mut locked_ws, _commit) = workspace_command.start_working_copy_mutation()?;
            let Some(locked_local_wc): Option<&mut LockedLocalWorkingCopy> =
                locked_ws.locked_wc().as_any_mut().downcast_mut()
            else {
                return Err(user_error(
                    "This command requires a standard local-disk working copy",
                ));
            };
            locked_local_wc.reset_watchman()?;
            locked_ws.finish(repo.op_id().clone())?;
            writeln!(ui.stderr(), "Reset Watchman clock")?;
        }
    }
    Ok(())
}

#[cfg(not(feature = "watchman"))]
fn cmd_debug_watchman(
    _ui: &mut Ui,
    _command: &CommandHelper,
    _subcommand: &DebugWatchmanSubcommand,
) -> Result<(), CommandError> {
    Err(user_error(
        "Cannot query Watchman because jj was not compiled with the `watchman` feature",
    ))
}

fn check_local_disk_wc(x: &dyn Any) -> Result<&LocalWorkingCopy, CommandError> {
    x.downcast_ref()
        .ok_or_else(|| user_error("This command requires a standard local-disk working copy"))
}
