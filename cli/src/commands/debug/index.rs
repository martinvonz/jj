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

use jj_lib::default_index::{AsCompositeIndex as _, DefaultReadonlyIndex};
use jj_lib::op_walk;

use crate::cli_util::CommandHelper;
use crate::command_error::{internal_error, user_error, CommandError};
use crate::ui::Ui;

/// Show commit index stats
#[derive(clap::Args, Clone, Debug)]
pub struct IndexArgs {}

pub fn cmd_debug_index(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &IndexArgs,
) -> Result<(), CommandError> {
    // Resolve the operation without loading the repo, so this command won't
    // merge concurrent operations and update the index.
    let workspace = command.load_workspace()?;
    let repo_loader = workspace.repo_loader();
    let op = op_walk::resolve_op_for_load(repo_loader, &command.global_args().at_operation)?;
    let index_store = repo_loader.index_store();
    let index = index_store
        .get_index_at_op(&op, repo_loader.store())
        .map_err(internal_error)?;
    if let Some(default_index) = index.as_any().downcast_ref::<DefaultReadonlyIndex>() {
        let stats = default_index.as_composite().stats();
        writeln!(ui.stdout(), "Number of commits: {}", stats.num_commits)?;
        writeln!(ui.stdout(), "Number of merges: {}", stats.num_merges)?;
        writeln!(
            ui.stdout(),
            "Max generation number: {}",
            stats.max_generation_number
        )?;
        writeln!(ui.stdout(), "Number of heads: {}", stats.num_heads)?;
        writeln!(ui.stdout(), "Number of changes: {}", stats.num_changes)?;
        writeln!(ui.stdout(), "Stats per level:")?;
        for (i, level) in stats.levels.iter().enumerate() {
            writeln!(ui.stdout(), "  Level {i}:")?;
            writeln!(ui.stdout(), "    Number of commits: {}", level.num_commits)?;
            writeln!(ui.stdout(), "    Name: {}", level.name.as_ref().unwrap())?;
        }
    } else {
        return Err(user_error(format!(
            "Cannot get stats for indexes of type '{}'",
            index_store.name()
        )));
    }
    Ok(())
}
