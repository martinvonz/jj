// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use tracing::instrument;

use super::new;
use crate::cli_util::{CommandError, CommandHelper};
use crate::ui::Ui;

#[instrument(skip_all)]
pub(crate) fn cmd_merge(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &new::NewArgs,
) -> Result<(), CommandError> {
    writeln!(
        ui.warning(),
        "warning: `jj merge` is deprecated; use `jj new` instead, which is equivalent"
    )?;
    writeln!(
        ui.warning(),
        "warning: `jj merge` will be removed in a future version, and this will be a hard error"
    )?;
    if args.revisions.len() < 2 {
        return Err(CommandError::CliError(String::from(
            "Merge requires at least two revisions",
        )));
    }
    new::cmd_new(ui, command, args)
}
