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

use jj_lib::git;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::git_util::print_failed_git_export;
use crate::ui::Ui;

/// Update the underlying Git repo with changes made in the repo
#[derive(clap::Args, Clone, Debug)]
pub struct ExportArgs {}

pub fn cmd_git_export(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &ExportArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let mut tx = workspace_command.start_transaction();
    let failed_branches = git::export_refs(tx.mut_repo())?;
    tx.finish(ui, "export git refs")?;
    print_failed_git_export(ui, &failed_branches)?;
    Ok(())
}
