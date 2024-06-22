// Copyright 2020-2023 The Jujutsu Authors
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
mod log;
mod restore;
pub mod undo;

use abandon::{cmd_op_abandon, OperationAbandonArgs};
use clap::Subcommand;
use log::{cmd_op_log, OperationLogArgs};
use restore::{cmd_op_restore, OperationRestoreArgs};
use undo::{cmd_op_undo, OperationUndoArgs};

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Commands for working with the operation log
///
/// For information about the operation log, see
/// https://github.com/martinvonz/jj/blob/main/docs/operation-log.md.
#[derive(Subcommand, Clone, Debug)]
pub enum OperationCommand {
    Abandon(OperationAbandonArgs),
    Log(OperationLogArgs),
    Undo(OperationUndoArgs),
    Restore(OperationRestoreArgs),
}

pub fn cmd_operation(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &OperationCommand,
) -> Result<(), CommandError> {
    match subcommand {
        OperationCommand::Abandon(args) => cmd_op_abandon(ui, command, args),
        OperationCommand::Log(args) => cmd_op_log(ui, command, args),
        OperationCommand::Restore(args) => cmd_op_restore(ui, command, args),
        OperationCommand::Undo(args) => cmd_op_undo(ui, command, args),
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum UndoWhatToRestore {
    /// The jj repo state and local branches
    Repo,
    /// The remote-tracking branches. Do not restore these if you'd like to push
    /// after the undo
    RemoteTracking,
}

pub const DEFAULT_UNDO_WHAT: [UndoWhatToRestore; 2] =
    [UndoWhatToRestore::Repo, UndoWhatToRestore::RemoteTracking];

/// Restore only the portions of the view specified by the `what` argument
fn view_with_desired_portions_restored(
    view_being_restored: &jj_lib::op_store::View,
    current_view: &jj_lib::op_store::View,
    what: &[UndoWhatToRestore],
) -> jj_lib::op_store::View {
    let repo_source = if what.contains(&UndoWhatToRestore::Repo) {
        view_being_restored
    } else {
        current_view
    };
    let remote_source = if what.contains(&UndoWhatToRestore::RemoteTracking) {
        view_being_restored
    } else {
        current_view
    };
    jj_lib::op_store::View {
        head_ids: repo_source.head_ids.clone(),
        local_branches: repo_source.local_branches.clone(),
        tags: repo_source.tags.clone(),
        remote_views: remote_source.remote_views.clone(),
        git_refs: current_view.git_refs.clone(),
        git_head: current_view.git_head.clone(),
        wc_commit_ids: repo_source.wc_commit_ids.clone(),
    }
}
