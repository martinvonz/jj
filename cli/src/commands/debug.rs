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
use jj_lib::backend::ObjectId;
use jj_lib::default_index_store::{DefaultIndexStore, ReadonlyIndexWrapper};
use jj_lib::local_working_copy::{LocalWorkingCopy, LockedLocalWorkingCopy};
use jj_lib::revset;
use jj_lib::working_copy::{LockedWorkingCopy, WorkingCopy};

use crate::cli_util::{resolve_op_for_load, user_error, CommandError, CommandHelper};
use crate::template_parser;
use crate::ui::Ui;

/// Low-level commands not intended for users
#[derive(Subcommand, Clone, Debug)]
#[command(hide = true)]
pub enum DebugCommands {
    Revset(DebugRevsetArgs),
    #[command(name = "workingcopy")]
    WorkingCopy(DebugWorkingCopyArgs),
    Template(DebugTemplateArgs),
    Index(DebugIndexArgs),
    #[command(name = "reindex")]
    ReIndex(DebugReIndexArgs),
    #[command(visible_alias = "view")]
    Operation(DebugOperationArgs),
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

#[derive(Subcommand, Clone, Debug)]
pub enum DebugWatchmanSubcommand {
    QueryClock,
    QueryChangedFiles,
    ResetClock,
}

pub fn cmd_debug(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &DebugCommands,
) -> Result<(), CommandError> {
    match subcommand {
        DebugCommands::Revset(args) => cmd_debug_revset(ui, command, args)?,
        DebugCommands::WorkingCopy(_wc_matches) => {
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
        }
        DebugCommands::Template(template_matches) => {
            let node = template_parser::parse_template(&template_matches.template)?;
            writeln!(ui.stdout(), "{node:#?}")?;
        }
        DebugCommands::Index(_index_matches) => {
            let workspace_command = command.workspace_helper(ui)?;
            let repo = workspace_command.repo();
            let index_impl: Option<&ReadonlyIndexWrapper> =
                repo.readonly_index().as_any().downcast_ref();
            if let Some(index_impl) = index_impl {
                let stats = index_impl.as_composite().stats();
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
                    repo.index_store().name()
                )));
            }
        }
        DebugCommands::ReIndex(_reindex_matches) => {
            let workspace_command = command.workspace_helper(ui)?;
            let repo = workspace_command.repo();
            let default_index_store: Option<&DefaultIndexStore> =
                repo.index_store().as_any().downcast_ref();
            if let Some(default_index_store) = default_index_store {
                default_index_store.reinit();
                let repo = repo.reload_at(repo.operation())?;
                let index_impl: &ReadonlyIndexWrapper = repo
                    .readonly_index()
                    .as_any()
                    .downcast_ref()
                    .expect("Default index should be a ReadonlyIndexWrapper");
                writeln!(
                    ui.stderr(),
                    "Finished indexing {:?} commits.",
                    index_impl.as_composite().stats().num_commits
                )?;
            } else {
                return Err(user_error(format!(
                    "Cannot reindex indexes of type '{}'",
                    repo.index_store().name()
                )));
            }
        }
        DebugCommands::Operation(operation_args) => {
            // Resolve the operation without loading the repo, so this command can be used
            // even if e.g. the view object is broken.
            let workspace = command.load_workspace()?;
            let repo_loader = workspace.repo_loader();
            let op = resolve_op_for_load(
                repo_loader.op_store(),
                repo_loader.op_heads_store(),
                &operation_args.operation,
            )?;
            if operation_args.display == DebugOperationDisplay::Id {
                writeln!(ui.stdout(), "{}", op.id().hex())?;
                return Ok(());
            }
            if operation_args.display != DebugOperationDisplay::View {
                writeln!(ui.stdout(), "{:#?}", op.store_operation())?;
            }
            if operation_args.display != DebugOperationDisplay::Operation {
                writeln!(ui.stdout(), "{:#?}", op.view()?.store_view())?;
            }
        }
        DebugCommands::Watchman(watchman_subcommand) => {
            cmd_debug_watchman(ui, command, watchman_subcommand)?;
        }
    }
    Ok(())
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

    let symbol_resolver = workspace_command.revset_symbol_resolver()?;
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

#[cfg(feature = "watchman")]
fn cmd_debug_watchman(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &DebugWatchmanSubcommand,
) -> Result<(), CommandError> {
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
