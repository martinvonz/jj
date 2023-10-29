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

use clap::ArgGroup;
use jj_lib::backend::ObjectId;
use jj_lib::repo::Repo;
use jj_lib::rewrite::merge_commit_trees;
use tracing::instrument;

use super::combine_messages;
use crate::cli_util::{user_error, CommandError, CommandHelper, RevisionArg};
use crate::ui::Ui;

/// Move changes from one revision into another
///
/// Use `--interactive` to move only part of the source revision into the
/// destination. The selected changes (or all the changes in the source revision
/// if not using `--interactive`) will be moved into the destination. The
/// changes will be removed from the source. If that means that the source is
/// now empty compared to its parent, it will be abandoned. Without
/// `--interactive`, the source change will always be empty.
///
/// If the source became empty and both the source and destination had a
/// non-empty description, you will be asked for the combined description. If
/// either was empty, then the other one will be used.
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("to_move").args(&["from", "to"]).multiple(true).required(true)))]
pub(crate) struct MoveArgs {
    /// Move part of this change into the destination
    #[arg(long)]
    from: Option<RevisionArg>,
    /// Move part of the source into this change
    #[arg(long)]
    to: Option<RevisionArg>,
    /// Interactively choose which parts to move
    #[arg(long, short)]
    interactive: bool,
    /// Move only changes to these paths (instead of all paths)
    #[arg(conflicts_with = "interactive", value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_move(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &MoveArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let source = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"), ui)?;
    let mut destination =
        workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"), ui)?;
    if source.id() == destination.id() {
        return Err(user_error("Source and destination cannot be the same."));
    }
    workspace_command.check_rewritable([&source, &destination])?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let mut tx = workspace_command.start_transaction(&format!(
        "move changes from {} to {}",
        source.id().hex(),
        destination.id().hex()
    ));
    let parent_tree = merge_commit_trees(tx.repo(), &source.parents())?;
    let source_tree = source.tree()?;
    let instructions = format!(
        "\
You are moving changes from: {}
into commit: {}

The left side of the diff shows the contents of the parent commit. The
right side initially shows the contents of the commit you're moving
changes from.

Adjust the right side until the diff shows the changes you want to move
to the destination. If you don't make any changes, then all the changes
from the source will be moved into the destination.
",
        tx.format_commit_summary(&source),
        tx.format_commit_summary(&destination)
    );
    let new_parent_tree_id = tx.select_diff(
        ui,
        &parent_tree,
        &source_tree,
        matcher.as_ref(),
        &instructions,
        args.interactive,
    )?;
    if args.interactive && new_parent_tree_id == parent_tree.id() {
        return Err(user_error("No changes to move"));
    }
    let new_parent_tree = tx.repo().store().get_root_tree(&new_parent_tree_id)?;
    // Apply the reverse of the selected changes onto the source
    let new_source_tree = source_tree.merge(&new_parent_tree, &parent_tree)?;
    let abandon_source = new_source_tree.id() == parent_tree.id();
    if abandon_source {
        tx.mut_repo().record_abandoned_commit(source.id().clone());
    } else {
        tx.mut_repo()
            .rewrite_commit(command.settings(), &source)
            .set_tree_id(new_source_tree.id().clone())
            .write()?;
    }
    if tx.repo().index().is_ancestor(source.id(), destination.id()) {
        // If we're moving changes to a descendant, first rebase descendants onto the
        // rewritten source. Otherwise it will likely already have the content
        // changes we're moving, so applying them will have no effect and the
        // changes will disappear.
        let mut rebaser = tx.mut_repo().create_descendant_rebaser(command.settings());
        rebaser.rebase_all()?;
        let rebased_destination_id = rebaser.rebased().get(destination.id()).unwrap().clone();
        destination = tx.mut_repo().store().get_commit(&rebased_destination_id)?;
    }
    // Apply the selected changes onto the destination
    let destination_tree = destination.tree()?;
    let new_destination_tree = destination_tree.merge(&parent_tree, &new_parent_tree)?;
    let description = combine_messages(
        tx.base_repo(),
        &source,
        &destination,
        command.settings(),
        abandon_source,
    )?;
    tx.mut_repo()
        .rewrite_commit(command.settings(), &destination)
        .set_tree_id(new_destination_tree.id().clone())
        .set_description(description)
        .write()?;
    tx.finish(ui)?;
    Ok(())
}
