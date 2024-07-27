// Copyright 2024 The Jujutsu Authors
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

use std::fs;
use std::io::Write;

use crate::cli_util::CommandHelper;
use crate::command_error::{CommandError, CommandErrorKind};
use crate::ui::Ui;

///  Removes files not tracked by Jujutsu
/// Note: snapshot won't be taken before purging, so there is no way to undo
/// this operation
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct PurgeArgs {
    /// Do actual removal of files, instead of just listing them
    #[arg(short, long, default_value = "false")]
    no_dry_run: bool,
}

pub(crate) fn cmd_purge(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &PurgeArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper_no_snapshot(ui)?;
    let snapshot = workspace_command.maybe_snapshot(ui)?;

    writeln!(ui.status(), "Purging files not tracked by Jujutsu")?;
    let max_snapshot_size = snapshot.files_to_large().first().map(|x| x.max_size);

    if let Some(max_size) = max_snapshot_size {
        writeln!(ui.status(), "Max allowed snapshot size: {}", max_size)?;
    }

    for path in snapshot.files_to_large() {
        writeln!(
            ui.status(),
            "File: {}, size: {}",
            path.path.display(),
            path.size
        )?;

        if args.no_dry_run {
            fs::remove_file(&path.path).map_err(|e| {
                CommandError::new(
                    CommandErrorKind::Cli,
                    format!("failed to remove {}: {}", path.path.display(), e),
                )
            })?;
        }
    }

    Ok(())
}
