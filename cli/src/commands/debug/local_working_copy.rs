// Copyright 2023 The Jujutsu Authors
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

use std::fmt::Debug;
use std::io::Write as _;

use jj_lib::working_copy::WorkingCopy;

use super::check_local_disk_wc;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Show information about the local working copy state
///
/// This command only works with a standard local-disk working copy.
#[derive(clap::Args, Clone, Debug)]
pub struct DebugLocalWorkingCopyArgs {}

pub fn cmd_debug_local_working_copy(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &DebugLocalWorkingCopyArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let wc = check_local_disk_wc(workspace_command.working_copy().as_any())?;
    writeln!(ui.stdout(), "Current operation: {:?}", wc.operation_id())?;
    writeln!(ui.stdout(), "Current tree: {:?}", wc.tree_id()?)?;
    for (file, state) in wc.file_states()? {
        writeln!(
            ui.stdout(),
            "{:?} {:13?} {:10?} {:?} {:?}",
            state.file_type,
            state.size,
            state.mtime.0,
            state.materialized_conflict_data,
            file
        )?;
    }
    Ok(())
}
