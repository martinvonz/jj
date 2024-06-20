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

use jj_lib::op_store::RefTarget;
use jj_lib::str_util::StringPattern;

use super::{find_local_branches, make_branch_term};
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Delete an existing branch and propagate the deletion to remotes on the
/// next push.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchDeleteArgs {
    /// The branches to delete
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select branches by wildcard pattern. For details, see
    /// https://github.com/martinvonz/jj/blob/main/docs/revsets.md#string-patterns.
    #[arg(required_unless_present_any(&["glob"]), value_parser = StringPattern::parse)]
    pub names: Vec<StringPattern>,

    /// Deprecated. Please prefix the pattern with `glob:` instead.
    #[arg(long, hide = true, value_parser = StringPattern::glob)]
    pub glob: Vec<StringPattern>,
}

pub fn cmd_branch_delete(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchDeleteArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    if !args.glob.is_empty() {
        writeln!(
            ui.warning_default(),
            "--glob has been deprecated. Please prefix the pattern with `glob:` instead."
        )?;
    }
    let name_patterns = [&args.names[..], &args.glob[..]].concat();
    let names = find_local_branches(view, &name_patterns)?;
    let mut tx = workspace_command.start_transaction();
    for branch_name in names.iter() {
        tx.mut_repo()
            .set_local_branch_target(branch_name, RefTarget::absent());
    }
    tx.finish(ui, format!("delete {}", make_branch_term(&names)))?;
    if names.len() > 1 {
        writeln!(ui.status(), "Deleted {} branches.", names.len())?;
    }
    Ok(())
}
