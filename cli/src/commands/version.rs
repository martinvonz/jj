// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use std::io::Write;

use tracing::instrument;

use crate::cli_util::{CommandError, CommandHelper};
use crate::ui::Ui;

/// Display version information
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct VersionArgs {}

#[instrument(skip_all)]
pub(crate) fn cmd_version(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &VersionArgs,
) -> Result<(), CommandError> {
    write!(ui.stdout(), "{}", command.app().render_version())?;
    Ok(())
}
