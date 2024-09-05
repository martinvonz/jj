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

use itertools::Itertools as _;
use jj_lib::op_store::RefTarget;
use jj_lib::str_util::StringPattern;

use super::find_local_branches;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Delete an existing branch and propagate the deletion to remotes on the
/// next push
#[derive(clap::Args, Clone, Debug)]
pub struct BranchDeleteArgs {
    /// The branches to delete
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select branches by wildcard pattern. For details, see
    /// https://martinvonz.github.io/jj/latest/revsets/#string-patterns.
    #[arg(required = true, value_parser = StringPattern::parse)]
    names: Vec<StringPattern>,
}

pub fn cmd_branch_delete(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchDeleteArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo().clone();
    let matched_branches = find_local_branches(repo.view(), &args.names)?;
    let mut tx = workspace_command.start_transaction();
    for (name, _) in &matched_branches {
        tx.mut_repo()
            .set_local_branch_target(name, RefTarget::absent());
    }
    writeln!(ui.status(), "Deleted {} branches.", matched_branches.len())?;
    tx.finish(
        ui,
        format!(
            "delete branch {}",
            matched_branches.iter().map(|(name, _)| name).join(", ")
        ),
    )?;
    Ok(())
}
