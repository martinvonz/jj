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

mod add;
mod forget;
mod list;
mod rename;
mod root;
mod update_stale;

use clap::Subcommand;
use tracing::instrument;

use self::add::cmd_workspace_add;
use self::add::WorkspaceAddArgs;
use self::forget::cmd_workspace_forget;
use self::forget::WorkspaceForgetArgs;
use self::list::cmd_workspace_list;
use self::list::WorkspaceListArgs;
use self::rename::cmd_workspace_rename;
use self::rename::WorkspaceRenameArgs;
use self::root::cmd_workspace_root;
use self::root::WorkspaceRootArgs;
use self::update_stale::cmd_workspace_update_stale;
use self::update_stale::WorkspaceUpdateStaleArgs;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Commands for working with workspaces
///
/// Workspaces let you add additional working copies attached to the same repo.
/// A common use case is so you can run a slow build or test in one workspace
/// while you're continuing to write code in another workspace.
///
/// Each workspace has its own working-copy commit. When you have more than one
/// workspace attached to a repo, they are indicated by `<workspace name>@` in
/// `jj log`.
///
/// Each workspace also has own sparse patterns.
#[derive(Subcommand, Clone, Debug)]
pub(crate) enum WorkspaceCommand {
    Add(WorkspaceAddArgs),
    Forget(WorkspaceForgetArgs),
    List(WorkspaceListArgs),
    Rename(WorkspaceRenameArgs),
    Root(WorkspaceRootArgs),
    UpdateStale(WorkspaceUpdateStaleArgs),
}

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
        WorkspaceCommand::Rename(args) => cmd_workspace_rename(ui, command, args),
        WorkspaceCommand::Root(args) => cmd_workspace_root(ui, command, args),
        WorkspaceCommand::UpdateStale(args) => cmd_workspace_update_stale(ui, command, args),
    }
}
