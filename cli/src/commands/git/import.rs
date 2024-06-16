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
use crate::git_util::print_git_import_stats;
use crate::ui::Ui;

/// Update repo with changes made in the underlying Git repo
///
/// If a working-copy commit gets abandoned, it will be given a new, empty
/// commit. This is true in general; it is not specific to this command.
#[derive(clap::Args, Clone, Debug)]
pub struct Args {}

pub fn run(ui: &mut Ui, command: &CommandHelper, _args: &Args) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let mut tx = workspace_command.start_transaction();
    // In non-colocated repo, HEAD@git will never be moved internally by jj.
    // That's why cmd_git_export() doesn't export the HEAD ref.
    git::import_head(tx.mut_repo())?;
    let stats = git::import_refs(tx.mut_repo(), &command.settings().git_settings())?;
    print_git_import_stats(ui, tx.repo(), &stats, true)?;
    tx.finish(ui, "import git refs")?;
    Ok(())
}
