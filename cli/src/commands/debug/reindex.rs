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

use jj_lib::default_index::{AsCompositeIndex as _, DefaultIndexStore};
use jj_lib::op_walk;

use crate::cli_util::CommandHelper;
use crate::command_error::{internal_error, user_error, CommandError};
use crate::ui::Ui;

/// Rebuild commit index
#[derive(clap::Args, Clone, Debug)]
pub struct DebugReindexArgs {}

pub fn cmd_debug_reindex(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &DebugReindexArgs,
) -> Result<(), CommandError> {
    // Resolve the operation without loading the repo. The index might have to
    // be rebuilt while loading the repo.
    let workspace = command.load_workspace()?;
    let repo_loader = workspace.repo_loader();
    let op = op_walk::resolve_op_for_load(repo_loader, &command.global_args().at_operation)?;
    let index_store = repo_loader.index_store();
    if let Some(default_index_store) = index_store.as_any().downcast_ref::<DefaultIndexStore>() {
        default_index_store.reinit().map_err(internal_error)?;
        let default_index = default_index_store
            .build_index_at_operation(&op, repo_loader.store())
            .map_err(internal_error)?;
        writeln!(
            ui.status(),
            "Finished indexing {:?} commits.",
            default_index.as_composite().stats().num_commits
        )?;
    } else {
        return Err(user_error(format!(
            "Cannot reindex indexes of type '{}'",
            index_store.name()
        )));
    }
    Ok(())
}
