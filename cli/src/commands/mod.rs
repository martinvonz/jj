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
mod unsquash;
mod untrack;
mod util;
mod version;
mod workspace;

use std::fmt;
use std::fmt::Debug;

use clap::{Command, CommandFactory, FromArgMatches, Subcommand};
use itertools::Itertools;
use tracing::instrument;

use crate::cli_util::{Args, CommandError, CommandHelper};
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
    Unsquash(unsquash::UnsquashArgs),
    Untrack(untrack::UntrackArgs),
    Version(version::VersionArgs),
    #[command(subcommand)]
    Workspace(workspace::WorkspaceCommands),
}

fn make_branch_term(branch_names: &[impl fmt::Display]) -> String {
    match branch_names {
        [branch_name] => format!("branch {}", branch_name),
        branch_names => format!("branches {}", branch_names.iter().join(", ")),
    }
}

pub fn default_app() -> Command {
    Commands::augment_subcommands(Args::command())
}

#[instrument(skip_all)]
pub fn run_command(ui: &mut Ui, command_helper: &CommandHelper) -> Result<(), CommandError> {
    let derived_subcommands: Commands =
        Commands::from_arg_matches(command_helper.matches()).unwrap();
    match &derived_subcommands {
        Commands::Version(sub_args) => version::cmd_version(ui, command_helper, sub_args),
        Commands::Init(sub_args) => init::cmd_init(ui, command_helper, sub_args),
        Commands::Config(sub_args) => config::cmd_config(ui, command_helper, sub_args),
        Commands::Checkout(sub_args) => checkout::cmd_checkout(ui, command_helper, sub_args),
        Commands::Untrack(sub_args) => untrack::cmd_untrack(ui, command_helper, sub_args),
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
        Commands::Unsquash(sub_args) => unsquash::cmd_unsquash(ui, command_helper, sub_args),
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
        Commands::Workspace(sub_args) => workspace::cmd_workspace(ui, command_helper, sub_args),
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
