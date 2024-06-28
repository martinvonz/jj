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

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Show information about the working copy state
#[derive(clap::Args, Clone, Debug)]
pub struct DebugWorkingCopyArgs {}

pub fn cmd_debug_working_copy(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &DebugWorkingCopyArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper_no_snapshot(ui)?;
    let wc = workspace_command.working_copy();
    writeln!(ui.stdout(), "Type: {:?}", wc.name())?;
    writeln!(ui.stdout(), "Current operation: {:?}", wc.operation_id())?;
    writeln!(ui.stdout(), "Current tree: {:?}", wc.tree_id()?)?;
    Ok(())
}
