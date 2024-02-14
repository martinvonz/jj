// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use tracing::instrument;

use crate::cli_util::{CommandError, CommandHelper};
use crate::commands::workspace;
use crate::ui::Ui;

/// Show the current workspace root directory
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct RootArgs {}

#[instrument(skip_all)]
pub(crate) fn cmd_root(
    ui: &mut Ui,
    command: &CommandHelper,
    RootArgs {}: &RootArgs,
) -> Result<(), CommandError> {
    workspace::cmd_workspace(
        ui,
        command,
        &workspace::WorkspaceCommand::Root(workspace::WorkspaceRootArgs {}),
    )
}
