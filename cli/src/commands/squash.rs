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

use jj_lib::commit::Commit;
use jj_lib::matchers::Matcher;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::Repo;
use jj_lib::revset;
use jj_lib::rewrite::merge_commit_trees;
use jj_lib::settings::UserSettings;
use tracing::instrument;

use crate::cli_util::{CommandHelper, DiffSelector, RevisionArg, WorkspaceCommandTransaction};
use crate::command_error::{user_error, CommandError};
use crate::description_util::{combine_messages, join_message_paragraphs};
use crate::ui::Ui;

/// Move changes from a revision into another revision
///
/// With the `-r` option, moves the changes from the specified revision to the
/// parent revision. Fails if there are several parent revisions (i.e., the
/// given revision is a merge).
///
/// With the `--from` and/or `--into` options, moves changes from/to the given
/// revisions. If either is left out, it defaults to the working-copy commit.
/// For example, `jj squash --into @--` moves changes from the working-copy
/// commit to the grandparent.
///
/// If, after moving changes out, the source revision is empty compared to its
/// parent(s), it will be abandoned. Without `--interactive`, the source
/// revision will always be empty.
///
/// If the source became empty and both the source and destination had a
/// non-empty description, you will be asked for the combined description. If
/// either was empty, then the other one will be used.
///
/// If a working-copy commit gets abandoned, it will be given a new, empty
/// commit. This is true in general; it is not specific to this command.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct SquashArgs {
    /// Revision to squash into its parent (default: @)
    #[arg(long, short)]
    revision: Option<RevisionArg>,
    /// Revision to squash from (default: @)
    #[arg(long, conflicts_with = "revision")]
    from: Option<RevisionArg>,
    /// Revision to squash into (default: @)
    #[arg(long, conflicts_with = "revision")]
    into: Option<RevisionArg>,
    /// The description to use for squashed revision (don't open editor)
    #[arg(long = "message", short, value_name = "MESSAGE")]
    message_paragraphs: Vec<String>,
    /// Interactively choose which parts to squash
    #[arg(long, short)]
    interactive: bool,
    /// Specify diff editor to be used (implies --interactive)
    #[arg(long, value_name = "NAME")]
    tool: Option<String>,
    /// Move only changes to these paths (instead of all paths)
    #[arg(conflicts_with_all = ["interactive", "tool"], value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_squash(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SquashArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let source;
    let destination;
    if args.from.is_some() || args.into.is_some() {
        source = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"))?;
        destination = workspace_command.resolve_single_rev(args.into.as_deref().unwrap_or("@"))?;
        if source.id() == destination.id() {
            return Err(user_error("Source and destination cannot be the same"));
        }
    } else {
        source = workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"))?;
        let mut parents = source.parents();
        if parents.len() != 1 {
            return Err(user_error("Cannot squash merge commits"));
        }
        destination = parents.pop().unwrap();
    }

    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let diff_selector =
        workspace_command.diff_selector(ui, args.tool.as_deref(), args.interactive)?;
    let mut tx = workspace_command.start_transaction();
    let tx_description = format!("squash commit {}", source.id().hex());
    let description = (!args.message_paragraphs.is_empty())
        .then(|| join_message_paragraphs(&args.message_paragraphs));
    move_diff(
        ui,
        &mut tx,
        command.settings(),
        source,
        destination,
        matcher.as_ref(),
        &diff_selector,
        description,
        args.revision.is_none() && args.from.is_none() && args.into.is_none(),
        &args.paths,
    )?;
    tx.finish(ui, tx_description)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn move_diff(
    ui: &mut Ui,
    tx: &mut WorkspaceCommandTransaction,
    settings: &UserSettings,
    source: Commit,
    mut destination: Commit,
    matcher: &dyn Matcher,
    diff_selector: &DiffSelector,
    description: Option<String>,
    no_rev_arg: bool,
    path_arg: &[String],
) -> Result<(), CommandError> {
    tx.base_workspace_helper()
        .check_rewritable([&source, &destination])?;
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
    let new_parent_tree_id =
        diff_selector.select(&parent_tree, &source_tree, matcher, Some(&instructions))?;
    if new_parent_tree_id == parent_tree.id() {
        if diff_selector.is_interactive() {
            return Err(user_error("No changes selected"));
        }

        if let [only_path] = path_arg {
            if no_rev_arg
                && revset::parse(
                    only_path,
                    &tx.base_workspace_helper().revset_parse_context(),
                )
                .is_ok()
            {
                writeln!(
                    ui.warning(),
                    "warning: The argument {only_path:?} is being interpreted as a path. To \
                     specify a revset, pass -r {only_path:?} instead."
                )?;
            }
        }
    }
    let new_parent_tree = tx.repo().store().get_root_tree(&new_parent_tree_id)?;
    // TODO: Do we want to optimize the case of moving to the parent commit (`jj
    // squash -r`)? The source tree will be unchanged in that case.

    // Apply the reverse of the selected changes onto the source
    let new_source_tree = source_tree.merge(&new_parent_tree, &parent_tree)?;
    let abandon_source = new_source_tree.id() == parent_tree.id();
    if abandon_source {
        tx.mut_repo().record_abandoned_commit(source.id().clone());
    } else {
        tx.mut_repo()
            .rewrite_commit(settings, &source)
            .set_tree_id(new_source_tree.id().clone())
            .write()?;
    }
    if tx.repo().index().is_ancestor(source.id(), destination.id()) {
        // If we're moving changes to a descendant, first rebase descendants onto the
        // rewritten source. Otherwise it will likely already have the content
        // changes we're moving, so applying them will have no effect and the
        // changes will disappear.
        let rebase_map = tx.mut_repo().rebase_descendants_return_map(settings)?;
        let rebased_destination_id = rebase_map.get(destination.id()).unwrap().clone();
        destination = tx.mut_repo().store().get_commit(&rebased_destination_id)?;
    }
    // Apply the selected changes onto the destination
    let destination_tree = destination.tree()?;
    let new_destination_tree = destination_tree.merge(&parent_tree, &new_parent_tree)?;
    let description = match description {
        Some(description) => description,
        None => {
            if abandon_source {
                combine_messages(
                    tx.base_repo(),
                    &source,
                    &destination,
                    settings,
                )?
            } else {
                destination.description().to_owned()
            }
        }
    };
    tx.mut_repo()
        .rewrite_commit(settings, &destination)
        .set_tree_id(new_destination_tree.id().clone())
        .set_predecessors(vec![destination.id().clone(), source.id().clone()])
        .set_description(description)
        .write()?;
    Ok(())
}
