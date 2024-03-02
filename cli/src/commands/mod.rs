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
    #[command(alias = "print")]
    Cat(cat::CatArgs),
    #[command(hide = true)]
    Checkout(checkout::CheckoutArgs),
    Chmod(chmod::ChmodArgs),
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
    Files(files::FilesArgs),
    #[command(subcommand)]
    Git(git::GitCommand),
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
    #[command(hide = true)]
    Merge(new::NewArgs),
    Move(r#move::MoveArgs),
    New(new::NewArgs),
    Next(next::NextArgs),
    Obslog(obslog::ObslogArgs),
    #[command(subcommand)]
    #[command(visible_alias = "op")]
    Operation(operation::OperationCommand),
    Prev(prev::PrevArgs),
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
    Sparse(sparse::SparseArgs),
    Split(split::SplitArgs),
    Squash(squash::SquashArgs),
    Status(status::StatusArgs),
    #[command(subcommand)]
    Tag(tag::TagCommand),
    #[command(subcommand)]
    Util(util::UtilCommand),
    /// Undo an operation (shortcut for `jj op undo`)
    Undo(operation::OperationUndoArgs),
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
    let derived_subcommands: Command = Command::from_arg_matches(command_helper.matches()).unwrap();
    match &derived_subcommands {
        Command::Version(sub_args) => version::cmd_version(ui, command_helper, sub_args),
        Command::Init(sub_args) => init::cmd_init(ui, command_helper, sub_args),
        Command::Config(sub_args) => config::cmd_config(ui, command_helper, sub_args),
        Command::Checkout(sub_args) => checkout::cmd_checkout(ui, command_helper, sub_args),
        Command::Untrack(sub_args) => untrack::cmd_untrack(ui, command_helper, sub_args),
        Command::Files(sub_args) => files::cmd_files(ui, command_helper, sub_args),
        Command::Cat(sub_args) => cat::cmd_cat(ui, command_helper, sub_args),
        Command::Diff(sub_args) => diff::cmd_diff(ui, command_helper, sub_args),
        Command::Show(sub_args) => show::cmd_show(ui, command_helper, sub_args),
        Command::Status(sub_args) => status::cmd_status(ui, command_helper, sub_args),
        Command::Log(sub_args) => log::cmd_log(ui, command_helper, sub_args),
        Command::Interdiff(sub_args) => interdiff::cmd_interdiff(ui, command_helper, sub_args),
        Command::Obslog(sub_args) => obslog::cmd_obslog(ui, command_helper, sub_args),
        Command::Describe(sub_args) => describe::cmd_describe(ui, command_helper, sub_args),
        Command::Commit(sub_args) => commit::cmd_commit(ui, command_helper, sub_args),
        Command::Duplicate(sub_args) => duplicate::cmd_duplicate(ui, command_helper, sub_args),
        Command::Abandon(sub_args) => abandon::cmd_abandon(ui, command_helper, sub_args),
        Command::Edit(sub_args) => edit::cmd_edit(ui, command_helper, sub_args),
        Command::Next(sub_args) => next::cmd_next(ui, command_helper, sub_args),
        Command::Prev(sub_args) => prev::cmd_prev(ui, command_helper, sub_args),
        Command::New(sub_args) => new::cmd_new(ui, command_helper, sub_args),
        Command::Move(sub_args) => r#move::cmd_move(ui, command_helper, sub_args),
        Command::Squash(sub_args) => squash::cmd_squash(ui, command_helper, sub_args),
        Command::Unsquash(sub_args) => unsquash::cmd_unsquash(ui, command_helper, sub_args),
        Command::Restore(sub_args) => restore::cmd_restore(ui, command_helper, sub_args),
        Command::Revert(_args) => revert(),
        Command::Root(sub_args) => root::cmd_root(ui, command_helper, sub_args),
        Command::Run(sub_args) => run::cmd_run(ui, command_helper, sub_args),
        Command::Diffedit(sub_args) => diffedit::cmd_diffedit(ui, command_helper, sub_args),
        Command::Split(sub_args) => split::cmd_split(ui, command_helper, sub_args),
        Command::Merge(sub_args) => merge::cmd_merge(ui, command_helper, sub_args),
        Command::Rebase(sub_args) => rebase::cmd_rebase(ui, command_helper, sub_args),
        Command::Backout(sub_args) => backout::cmd_backout(ui, command_helper, sub_args),
        Command::Resolve(sub_args) => resolve::cmd_resolve(ui, command_helper, sub_args),
        Command::Branch(sub_args) => branch::cmd_branch(ui, command_helper, sub_args),
        Command::Undo(sub_args) => operation::cmd_op_undo(ui, command_helper, sub_args),
        Command::Operation(sub_args) => operation::cmd_operation(ui, command_helper, sub_args),
        Command::Workspace(sub_args) => workspace::cmd_workspace(ui, command_helper, sub_args),
        Command::Sparse(sub_args) => sparse::cmd_sparse(ui, command_helper, sub_args),
        Command::Tag(sub_args) => tag::cmd_tag(ui, command_helper, sub_args),
        Command::Chmod(sub_args) => chmod::cmd_chmod(ui, command_helper, sub_args),
        Command::Git(sub_args) => git::cmd_git(ui, command_helper, sub_args),
        Command::Util(sub_args) => util::cmd_util(ui, command_helper, sub_args),
        #[cfg(feature = "bench")]
        Command::Bench(sub_args) => bench::cmd_bench(ui, command_helper, sub_args),
        Command::Debug(sub_args) => debug::cmd_debug(ui, command_helper, sub_args),
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
