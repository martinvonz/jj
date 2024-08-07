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

use std::error::Error;
use std::fs;
use std::io::Write;

use jj_lib::settings::HumanByteSize;
use jj_lib::working_copy::SnapshotError;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

///  Removes files not tracked by Jujutsu
/// Note: snapshot won't be taken before purging, so there is no way to undo
/// this operation
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct PurgeArgs {
    /// Dry run, don't actually remove files
    #[arg(short, long, default_value = "false")]
    dry_run: bool,
}

pub(crate) fn cmd_purge(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &PurgeArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui);
    if let Err(e) = workspace_command {
        let Some(e) = e.error.source() else {
            return Ok(());
        };
        let e = e.downcast_ref::<SnapshotError>();
        if let Some(SnapshotError::NewFileTooLarge(files)) = e {
            writeln!(
                ui.status(),
                "The following files are too large to be added to the working copy:"
            )?;
            for file in files {
                writeln!(ui.status(), "  {}", &file.path.display())?;
            }
            if !args.dry_run {
                for file in files {
                    fs::remove_file(&file.path)?;
                }
            }
            let total_size: u64 = files.iter().map(|file| file.size.0).sum();

            writeln!(
                ui.status(),
                "Removed {} files totaling {}",
                files.len(),
                HumanByteSize(total_size)
            )?;
        }
    }
    Ok(())
}
