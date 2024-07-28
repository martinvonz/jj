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

use itertools::Itertools;
use jj_lib::repo::Repo;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// List workspaces
#[derive(clap::Args, Clone, Debug)]
pub struct WorkspaceListArgs {}

#[instrument(skip_all)]
pub fn cmd_workspace_list(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let mut formatter = ui.stdout_formatter();
    let template = workspace_command.commit_summary_template();
    for (workspace_id, wc_commit_id) in repo.view().wc_commit_ids().iter().sorted() {
        write!(formatter, "{}: ", workspace_id.as_str())?;
        let commit = repo.store().get_commit(wc_commit_id)?;
        template.format(&commit, formatter.as_mut())?;
        writeln!(formatter)?;
    }
    Ok(())
}
