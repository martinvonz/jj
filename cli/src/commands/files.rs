// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use std::io::Write;

use tracing::instrument;

use crate::cli_util::{CommandError, CommandHelper, RevisionArg};
use crate::ui::Ui;

/// List files in a revision
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct FilesArgs {
    /// The revision to list files in
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Only list files matching these prefixes (instead of all files)
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_files(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &FilesArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    let tree = commit.tree()?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    ui.request_pager();
    for (name, _value) in tree.entries_matching(matcher.as_ref()) {
        writeln!(
            ui.stdout(),
            "{}",
            &workspace_command.format_file_path(&name)
        )?;
    }
    Ok(())
}
