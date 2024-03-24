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

    let mut sources;
    let destination;
    if args.from.is_some() || args.into.is_some() {
        sources = workspace_command.resolve_revset(args.from.as_deref().unwrap_or("@"))?;
        destination = workspace_command.resolve_single_rev(args.into.as_deref().unwrap_or("@"))?;
        if sources.iter().any(|source| source.id() == destination.id()) {
            return Err(user_error("Source and destination cannot be the same"));
        }
        // Reverse the set so we apply the oldest commits first. It shouldn't affect the
        // result, but it avoids creating transient conflicts and is therefore probably
        // a little faster.
        sources.reverse();
    } else {
        let source =
            workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"))?;
        let mut parents = source.parents();
        if parents.len() != 1 {
            return Err(user_error("Cannot squash merge commits"));
        }
        sources = vec![source];
        destination = parents.pop().unwrap();
    }

    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let diff_selector =
        workspace_command.diff_selector(ui, args.tool.as_deref(), args.interactive)?;
    let mut tx = workspace_command.start_transaction();
    let tx_description = format!("squash commits into {}", destination.id().hex());
    let description = (!args.message_paragraphs.is_empty())
        .then(|| join_message_paragraphs(&args.message_paragraphs));
    move_diff(
        ui,
        &mut tx,
        command.settings(),
        &sources,
        &destination,
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
    sources: &[Commit],
    destination: &Commit,
    matcher: &dyn Matcher,
    diff_selector: &DiffSelector,
    description: Option<String>,
    no_rev_arg: bool,
    path_arg: &[String],
) -> Result<(), CommandError> {
    tx.base_workspace_helper()
        .check_rewritable(sources.iter().chain(std::iter::once(destination)))?;
    // Tree diffs to apply to the destination
    let mut tree_diffs = vec![];
    let mut abandoned_commits = vec![];
    for source in sources {
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
            tx.format_commit_summary(source),
            tx.format_commit_summary(destination)
        );
        let new_parent_tree_id =
            diff_selector.select(&parent_tree, &source_tree, matcher, Some(&instructions))?;
        let new_parent_tree = tx.repo().store().get_root_tree(&new_parent_tree_id)?;
        // TODO: Do we want to optimize the case of moving to the parent commit (`jj
        // squash -r`)? The source tree will be unchanged in that case.

        // Apply the reverse of the selected changes onto the source
        let new_source_tree = source_tree.merge(&new_parent_tree, &parent_tree)?;
        let abandon_source = new_source_tree.id() == parent_tree.id();
        if abandon_source {
            abandoned_commits.push(source);
            tx.mut_repo().record_abandoned_commit(source.id().clone());
        } else {
            tx.mut_repo()
                .rewrite_commit(settings, source)
                .set_tree_id(new_source_tree.id().clone())
                .write()?;
        }
        if new_parent_tree_id != parent_tree.id() {
            tree_diffs.push((parent_tree, new_parent_tree));
        }
    }
    if tree_diffs.is_empty() {
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
                    ui.warning_default(),
                    "The argument {only_path:?} is being interpreted as a path. To specify a \
                     revset, pass -r {only_path:?} instead."
                )?;
            }
        }
    }
    let mut rewritten_destination = destination.clone();
    if sources
        .iter()
        .any(|source| tx.repo().index().is_ancestor(source.id(), destination.id()))
    {
        // If we're moving changes to a descendant, first rebase descendants onto the
        // rewritten sources. Otherwise it will likely already have the content
        // changes we're moving, so applying them will have no effect and the
        // changes will disappear.
        let rebase_map = tx.mut_repo().rebase_descendants_return_map(settings)?;
        let rebased_destination_id = rebase_map.get(destination.id()).unwrap().clone();
        rewritten_destination = tx.mut_repo().store().get_commit(&rebased_destination_id)?;
    }
    // Apply the selected changes onto the destination
    let mut destination_tree = rewritten_destination.tree()?;
    for (tree1, tree2) in tree_diffs {
        destination_tree = destination_tree.merge(&tree1, &tree2)?;
    }
    let description = match description {
        Some(description) => description,
        None => combine_messages(tx.base_repo(), &abandoned_commits, destination, settings)?,
    };
    let mut predecessors = vec![destination.id().clone()];
    predecessors.extend(sources.iter().map(|source| source.id().clone()));
    tx.mut_repo()
        .rewrite_commit(settings, &rewritten_destination)
        .set_tree_id(destination_tree.id().clone())
        .set_predecessors(predecessors)
        .set_description(description)
        .write()?;
    Ok(())
}
