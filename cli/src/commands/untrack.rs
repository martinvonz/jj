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

use std::io::Write;

use itertools::Itertools;
use jj_lib::merge::Merge;
use jj_lib::merged_tree::MergedTreeBuilder;
use jj_lib::repo::Repo;
use jj_lib::working_copy::SnapshotOptions;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error::{user_error_with_hint, CommandError};
use crate::ui::Ui;

/// Stop tracking specified paths in the working copy
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct UntrackArgs {
    /// Paths to untrack. They must already be ignored.
    ///
    /// The paths could be ignored via a .gitignore or .git/info/exclude (in
    /// colocated repos).
    #[arg(required = true, value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_untrack(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &UntrackArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let store = workspace_command.repo().store().clone();
    let matcher = workspace_command.matcher_from_values(&args.paths)?;

    let mut tx = workspace_command.start_transaction().into_inner();
    let base_ignores = workspace_command.base_ignores()?;
    let (mut locked_ws, wc_commit) = workspace_command.start_working_copy_mutation()?;
    // Create a new tree without the unwanted files
    let mut tree_builder = MergedTreeBuilder::new(wc_commit.tree_id().clone());
    let wc_tree = wc_commit.tree()?;
    for (path, _value) in wc_tree.entries_matching(matcher.as_ref()) {
        tree_builder.set_or_remove(path, Merge::absent());
    }
    let new_tree_id = tree_builder.write_tree(&store)?;
    let new_commit = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &wc_commit)
        .set_tree_id(new_tree_id)
        .write()?;
    // Reset the working copy to the new commit
    locked_ws.locked_wc().reset(&new_commit)?;
    // Commit the working copy again so we can inform the user if paths couldn't be
    // untracked because they're not ignored.
    let wc_tree_id = locked_ws.locked_wc().snapshot(SnapshotOptions {
        base_ignores,
        fsmonitor_kind: command.settings().fsmonitor_kind()?,
        progress: None,
        max_new_file_size: command.settings().max_new_file_size()?,
    })?;
    if wc_tree_id != *new_commit.tree_id() {
        let wc_tree = store.get_root_tree(&wc_tree_id)?;
        let added_back = wc_tree.entries_matching(matcher.as_ref()).collect_vec();
        if !added_back.is_empty() {
            drop(locked_ws);
            let path = &added_back[0].0;
            let ui_path = workspace_command.format_file_path(path);
            let message = if added_back.len() > 1 {
                format!(
                    "'{}' and {} other files are not ignored.",
                    ui_path,
                    added_back.len() - 1
                )
            } else {
                format!("'{ui_path}' is not ignored.")
            };
            return Err(user_error_with_hint(
                message,
                "Files that are not ignored will be added back by the next command.
Make sure they're ignored, then try again.",
            ));
        } else {
            // This means there were some concurrent changes made in the working copy. We
            // don't want to mix those in, so reset the working copy again.
            locked_ws.locked_wc().reset(&new_commit)?;
        }
    }
    let num_rebased = tx.mut_repo().rebase_descendants(command.settings())?;
    if num_rebased > 0 {
        writeln!(ui.stderr(), "Rebased {num_rebased} descendant commits")?;
    }
    let repo = tx.commit("untrack paths");
    locked_ws.finish(repo.op_id().clone())?;
    Ok(())
}
