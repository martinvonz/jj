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
use jj_lib::commit::Commit;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::Repo;
use jj_lib::revset::{RevsetExpression, RevsetIteratorExt};
use jj_lib::rewrite::merge_commit_trees;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::CommandError;
use crate::commands::rebase::rebase_descendants;
use crate::description_util::{description_template_for_commit, edit_description};
use crate::diff_util::DiffFormatArgs;
use crate::ui::Ui;

/// Split a revision in two
///
/// Starts a diff editor (`meld` by default) on the changes in the revision.
/// Edit the right side of the diff until it has the content you want in the
/// first revision. Once you close the editor, your edited content will replace
/// the previous revision. The remaining changes will be put in a new revision
/// on top.
///
/// If the change you split had a description, you will be asked to enter a
/// change description for each commit. If the change did not have a
/// description, the second part will not get a description, and you will be
/// asked for a description only for the first part.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct SplitArgs {
    /// Interactively choose which parts to split. This is the default if no
    /// paths are provided.
    #[arg(long, short)]
    interactive: bool,
    /// Specify diff editor to be used (implies --interactive)
    #[arg(long, value_name = "NAME")]
    tool: Option<String>,
    /// The revision to split
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Split the revision into two siblings instead of a parent and child.
    #[arg(long, short)]
    siblings: bool,
    /// Put these paths in the first commit
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
    #[command(flatten)]
    diff_format: DiffFormatArgs,
}

#[instrument(skip_all)]
pub(crate) fn cmd_split(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SplitArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewritable([commit.id()])?;
    let matcher = workspace_command
        .parse_file_patterns(&args.paths)?
        .to_matcher();
    let diff_selector = workspace_command.diff_selector(
        ui,
        args.tool.as_deref(),
        args.interactive || args.paths.is_empty(),
    )?;
    let mut tx = workspace_command.start_transaction();
    let end_tree = commit.tree()?;
    let base_tree = merge_commit_trees(tx.repo(), &commit.parents())?;
    let instructions = format!(
        "\
You are splitting a commit into two: {}

The diff initially shows the changes in the commit you're splitting.

Adjust the right side until it shows the contents you want for the first commit.
The remainder will be in the second commit. If you don't make any changes, then
the operation will be aborted.
",
        tx.format_commit_summary(&commit)
    );

    // Prompt the user to select the changes they want for the first commit.
    let selected_tree_id =
        diff_selector.select(&base_tree, &end_tree, matcher.as_ref(), Some(&instructions))?;
    if &selected_tree_id == commit.tree_id() && diff_selector.is_interactive() {
        // The user selected everything from the original commit.
        writeln!(ui.status(), "Nothing changed.")?;
        return Ok(());
    }
    if selected_tree_id == base_tree.id() {
        // The user selected nothing, so the first commit will be empty.
        writeln!(
            ui.warning_default(),
            "The given paths do not match any file: {}",
            args.paths.join(" ")
        )?;
    }

    // Create the first commit, which includes the changes selected by the user.
    let selected_tree = tx.repo().store().get_root_tree(&selected_tree_id)?;
    let first_template = description_template_for_commit(
        ui,
        command.settings(),
        tx.base_workspace_helper(),
        "Enter a description for the first commit.",
        commit.description(),
        &base_tree,
        &selected_tree,
        &args.diff_format,
    )?;
    let first_description = edit_description(tx.base_repo(), &first_template, command.settings())?;
    let first_commit = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_tree_id(selected_tree_id)
        .set_description(first_description)
        .write()?;

    // Create the second commit, which includes everything the user didn't
    // select.
    let (second_tree, second_base_tree) = if args.siblings {
        // Merge the original commit tree with its parent using the tree
        // containing the user selected changes as the base for the merge.
        // This results in a tree with the changes the user didn't select.
        (end_tree.merge(&selected_tree, &base_tree)?, &base_tree)
    } else {
        (end_tree, &selected_tree)
    };
    let second_commit_parents = if args.siblings {
        commit.parent_ids().to_vec()
    } else {
        vec![first_commit.id().clone()]
    };
    let second_description = if commit.description().is_empty() {
        // If there was no description before, don't ask for one for the second commit.
        "".to_string()
    } else {
        let second_template = description_template_for_commit(
            ui,
            command.settings(),
            tx.base_workspace_helper(),
            "Enter a description for the second commit.",
            commit.description(),
            second_base_tree,
            &second_tree,
            &args.diff_format,
        )?;
        edit_description(tx.base_repo(), &second_template, command.settings())?
    };
    let second_commit = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_parents(second_commit_parents)
        .set_tree_id(second_tree.id())
        // Generate a new change id so that the commit being split doesn't
        // become divergent.
        .generate_new_change_id()
        .set_description(second_description)
        .write()?;

    // Mark the commit being split as rewritten to the second commit. As a
    // result, if @ points to the commit being split, it will point to the
    // second commit after the command finishes. This also means that any
    // branches pointing to the commit being split are moved to the second
    // commit.
    tx.mut_repo()
        .set_rewritten_commit(commit.id().clone(), second_commit.id().clone());
    // Rebase descendants of the commit being split.
    let new_parents = if args.siblings {
        vec![first_commit.clone(), second_commit.clone()]
    } else {
        vec![second_commit.clone()]
    };
    let children: Vec<Commit> = RevsetExpression::commit(commit.id().clone())
        .children()
        .evaluate_programmatic(tx.base_repo().as_ref())?
        .iter()
        .commits(tx.base_repo().store())
        .try_collect()?;
    let num_rebased = rebase_descendants(
        &mut tx,
        command.settings(),
        &new_parents,
        &children,
        Default::default(),
    )?;
    if let Some(mut formatter) = ui.status_formatter() {
        if num_rebased > 0 {
            writeln!(formatter, "Rebased {num_rebased} descendant commits")?;
        }
        write!(formatter, "First part: ")?;
        tx.write_commit_summary(formatter.as_mut(), &first_commit)?;
        write!(formatter, "\nSecond part: ")?;
        tx.write_commit_summary(formatter.as_mut(), &second_commit)?;
        writeln!(formatter)?;
    }
    tx.finish(ui, format!("split commit {}", commit.id().hex()))?;
    Ok(())
}
