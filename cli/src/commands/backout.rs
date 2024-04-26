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

use jj_lib::object_id::ObjectId;
use jj_lib::rewrite::merge_commit_trees;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Apply the reverse of a revision on top of another revision
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct BackoutArgs {
    /// The revision to apply the reverse of
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// The revision to apply the reverse changes on top of
    // TODO: It seems better to default this to `@-`. Maybe the working
    // copy should be rebased on top?
    #[arg(long, short, default_value = "@")]
    destination: Vec<RevisionArg>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_backout(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BackoutArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit_to_back_out = workspace_command.resolve_single_rev(&args.revision)?;
    let mut parents = vec![];
    for revision_str in &args.destination {
        let destination = workspace_command.resolve_single_rev(revision_str)?;
        parents.push(destination);
    }
    let mut tx = workspace_command.start_transaction();
    let old_base_tree = commit_to_back_out.parent_tree(tx.mut_repo())?;
    let new_base_tree = merge_commit_trees(tx.mut_repo(), &parents)?;
    let old_tree = commit_to_back_out.tree()?;
    let new_tree = new_base_tree.merge(&old_tree, &old_base_tree)?;
    let new_parent_ids = parents.iter().map(|commit| commit.id().clone()).collect();
    tx.mut_repo()
        .new_commit(command.settings(), new_parent_ids, new_tree.id())
        .set_description(format!(
            "backout of commit {}",
            &commit_to_back_out.id().hex()
        ))
        .write()?;
    tx.finish(
        ui,
        format!("back out commit {}", commit_to_back_out.id().hex()),
    )?;

    Ok(())
}
