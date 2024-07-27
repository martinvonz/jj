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
mod checkout;
mod commit;
mod config;
mod debug;
mod describe;
mod diff;
mod diffedit;
mod duplicate;
mod edit;
mod file;
mod fix;
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
mod parallelize;
mod prev;
mod purge;
mod rebase;
mod resolve;
mod restore;
mod root;
mod run;
mod show;
mod sparse;
mod split;
mod squash;
mod status;
mod tag;
mod unsquash;
mod untrack;
mod util;
mod version;
mod workspace;

use std::fmt::Debug;

use clap::{CommandFactory, FromArgMatches, Subcommand};
use tracing::instrument;

use crate::cli_util::{Args, CommandHelper};
use crate::command_error::{user_error_with_hint, CommandError};
use crate::ui::Ui;

#[derive(clap::Parser, Clone, Debug)]
enum Command {
    Abandon(abandon::AbandonArgs),
    Backout(backout::BackoutArgs),
    #[cfg(feature = "bench")]
    #[command(subcommand)]
    Bench(bench::BenchCommand),
    #[command(subcommand)]
    Branch(branch::BranchCommand),
    #[command(alias = "print", hide = true)]
    Cat(file::show::FileShowArgs),
    #[command(hide = true)]
    Checkout(checkout::CheckoutArgs),
    #[command(hide = true)]
    Chmod(file::chmod::FileChmodArgs),
    Commit(commit::CommitArgs),
    #[command(subcommand)]
    Config(config::ConfigCommand),
    #[command(subcommand)]
    Debug(debug::DebugCommand),
    Describe(describe::DescribeArgs),
    Diff(diff::DiffArgs),
    Diffedit(diffedit::DiffeditArgs),
    Duplicate(duplicate::DuplicateArgs),
    Edit(edit::EditArgs),
    #[command(subcommand)]
    File(file::FileCommand),
    /// List files in a revision (DEPRECATED use `jj file list`)
    #[command(hide = true)]
    Files(file::list::FileListArgs),
    Fix(fix::FixArgs),
    #[command(subcommand)]
    Git(git::GitCommand),
    Init(init::InitArgs),
    Interdiff(interdiff::InterdiffArgs),
    Log(log::LogArgs),
    /// Merge work from multiple branches (DEPRECATED, use `jj new`)
    ///
    /// Unlike most other VCSs, `jj merge` does not implicitly include the
    /// working copy revision's parent as one of the parents of the merge;
    /// you need to explicitly list all revisions that should become parents
    /// of the merge.
    ///
    /// This is the same as `jj new`, except that it requires at least two
    /// arguments.
    #[command(hide = true)]
    Merge(new::NewArgs),
    #[command(hide = true)]
    Move(r#move::MoveArgs),
    New(new::NewArgs),
    Next(next::NextArgs),
    Obslog(obslog::ObslogArgs),
    #[command(subcommand)]
    #[command(visible_alias = "op")]
    Operation(operation::OperationCommand),
    Parallelize(parallelize::ParallelizeArgs),
    Prev(prev::PrevArgs),
    Purge(purge::PurgeArgs),
    Rebase(rebase::RebaseArgs),
    Resolve(resolve::ResolveArgs),
    Restore(restore::RestoreArgs),
    #[command(
        hide = true,
        help_template = "Not a real subcommand; consider `jj backout` or `jj restore`"
    )]
    Revert(DummyCommandArgs),
    Root(root::RootArgs),
    #[command(hide = true)]
    // TODO: Flesh out.
    Run(run::RunArgs),
    Show(show::ShowArgs),
    #[command(subcommand)]
    Sparse(sparse::SparseCommand),
    Split(split::SplitArgs),
    Squash(squash::SquashArgs),
    Status(status::StatusArgs),
    #[command(subcommand)]
    Tag(tag::TagCommand),
    #[command(subcommand)]
    Util(util::UtilCommand),
    /// Undo an operation (shortcut for `jj op undo`)
    Undo(operation::undo::OperationUndoArgs),
    Unsquash(unsquash::UnsquashArgs),
    Untrack(untrack::UntrackArgs),
    Version(version::VersionArgs),
    #[command(subcommand)]
    Workspace(workspace::WorkspaceCommand),
}

/// A dummy command that accepts any arguments
#[derive(clap::Args, Clone, Debug)]
struct DummyCommandArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    _args: Vec<String>,
}

pub fn default_app() -> clap::Command {
    Command::augment_subcommands(Args::command())
}

#[instrument(skip_all)]
pub fn run_command(ui: &mut Ui, command_helper: &CommandHelper) -> Result<(), CommandError> {
    let subcommand = Command::from_arg_matches(command_helper.matches()).unwrap();
    match &subcommand {
        Command::Abandon(args) => abandon::cmd_abandon(ui, command_helper, args),
        Command::Backout(args) => backout::cmd_backout(ui, command_helper, args),
        #[cfg(feature = "bench")]
        Command::Bench(args) => bench::cmd_bench(ui, command_helper, args),
        Command::Branch(args) => branch::cmd_branch(ui, command_helper, args),
        Command::Cat(args) => file::show::deprecated_cmd_cat(ui, command_helper, args),
        Command::Checkout(args) => checkout::cmd_checkout(ui, command_helper, args),
        Command::Chmod(args) => file::chmod::deprecated_cmd_chmod(ui, command_helper, args),
        Command::Commit(args) => commit::cmd_commit(ui, command_helper, args),
        Command::Config(args) => config::cmd_config(ui, command_helper, args),
        Command::Debug(args) => debug::cmd_debug(ui, command_helper, args),
        Command::Describe(args) => describe::cmd_describe(ui, command_helper, args),
        Command::Diff(args) => diff::cmd_diff(ui, command_helper, args),
        Command::Diffedit(args) => diffedit::cmd_diffedit(ui, command_helper, args),
        Command::Duplicate(args) => duplicate::cmd_duplicate(ui, command_helper, args),
        Command::Edit(args) => edit::cmd_edit(ui, command_helper, args),
        Command::File(args) => file::cmd_file(ui, command_helper, args),
        Command::Files(args) => file::list::deprecated_cmd_files(ui, command_helper, args),
        Command::Fix(args) => fix::cmd_fix(ui, command_helper, args),
        Command::Git(args) => git::cmd_git(ui, command_helper, args),
        Command::Init(args) => init::cmd_init(ui, command_helper, args),
        Command::Interdiff(args) => interdiff::cmd_interdiff(ui, command_helper, args),
        Command::Log(args) => log::cmd_log(ui, command_helper, args),
        Command::Merge(args) => merge::cmd_merge(ui, command_helper, args),
        Command::Move(args) => r#move::cmd_move(ui, command_helper, args),
        Command::New(args) => new::cmd_new(ui, command_helper, args),
        Command::Next(args) => next::cmd_next(ui, command_helper, args),
        Command::Obslog(args) => obslog::cmd_obslog(ui, command_helper, args),
        Command::Operation(args) => operation::cmd_operation(ui, command_helper, args),
        Command::Parallelize(args) => parallelize::cmd_parallelize(ui, command_helper, args),
        Command::Prev(args) => prev::cmd_prev(ui, command_helper, args),
        Command::Rebase(args) => rebase::cmd_rebase(ui, command_helper, args),
        Command::Resolve(args) => resolve::cmd_resolve(ui, command_helper, args),
        Command::Restore(args) => restore::cmd_restore(ui, command_helper, args),
        Command::Revert(_args) => revert(),
        Command::Purge(args) => purge::cmd_purge(ui, command_helper, args),
        Command::Root(args) => root::cmd_root(ui, command_helper, args),
        Command::Run(args) => run::cmd_run(ui, command_helper, args),
        Command::Show(args) => show::cmd_show(ui, command_helper, args),
        Command::Sparse(args) => sparse::cmd_sparse(ui, command_helper, args),
        Command::Split(args) => split::cmd_split(ui, command_helper, args),
        Command::Squash(args) => squash::cmd_squash(ui, command_helper, args),
        Command::Status(args) => status::cmd_status(ui, command_helper, args),
        Command::Tag(args) => tag::cmd_tag(ui, command_helper, args),
        Command::Undo(args) => operation::undo::cmd_op_undo(ui, command_helper, args),
        Command::Unsquash(args) => unsquash::cmd_unsquash(ui, command_helper, args),
        Command::Untrack(args) => untrack::cmd_untrack(ui, command_helper, args),
        Command::Util(args) => util::cmd_util(ui, command_helper, args),
        Command::Version(args) => version::cmd_version(ui, command_helper, args),
        Command::Workspace(args) => workspace::cmd_workspace(ui, command_helper, args),
    }
}

fn revert() -> Result<(), CommandError> {
    Err(user_error_with_hint(
        "No such subcommand: revert",
        "Consider `jj backout` or `jj restore`",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_app() {
        default_app().debug_assert();
    }
}
