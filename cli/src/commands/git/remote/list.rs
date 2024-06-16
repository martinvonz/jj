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

use std::io::Write;

use jj_lib::repo::Repo;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::git_util::get_git_repo;
use crate::ui::Ui;

/// List Git remotes
#[derive(clap::Args, Clone, Debug)]
pub struct ListArgs {}

pub fn cmd_remote_list(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &ListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    for remote_name in git_repo.remotes()?.iter().flatten() {
        let remote = git_repo.find_remote(remote_name)?;
        writeln!(
            ui.stdout(),
            "{} {}",
            remote_name,
            remote.url().unwrap_or("<no URL>")
        )?;
    }
    Ok(())
}
