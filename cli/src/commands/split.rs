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

use jj_lib::backend::ObjectId;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::merged_tree::MergedTree;
use jj_lib::repo::Repo;
use jj_lib::rewrite::{merge_commit_trees, DescendantRebaser};
use jj_lib::settings::UserSettings;
use maplit::{hashmap, hashset};
use tracing::instrument;

use crate::cli_util::{CommandError, CommandHelper, RevisionArg, WorkspaceCommandHelper};
use crate::description_util::{diff_summary_to_description, edit_description};
use crate::diff_util::{self, DiffFormat};
use crate::formatter::PlainTextFormatter;
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
    /// The revision to split
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Put these paths in the first commit
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_split(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SplitArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    workspace_command.check_rewritable([&commit])?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let mut tx =
        workspace_command.start_transaction(&format!("split commit {}", commit.id().hex()));
    let end_tree = commit.tree()?;
    let base_tree = merge_commit_trees(tx.repo(), &commit.parents())?;
    let interactive = args.interactive || args.paths.is_empty();
    let instructions = format!(
        "\
You are splitting a commit in two: {}

The diff initially shows the changes in the commit you're splitting.

Adjust the right side until it shows the contents you want for the first
(parent) commit. The remainder will be in the second commit. If you
don't make any changes, then the operation will be aborted.
",
        tx.format_commit_summary(&commit)
    );
    let tree_id = tx.select_diff(
        ui,
        &base_tree,
        &end_tree,
        matcher.as_ref(),
        &instructions,
        interactive,
    )?;
    if &tree_id == commit.tree_id() && interactive {
        writeln!(ui.stderr(), "Nothing changed.")?;
        return Ok(());
    }
    let middle_tree = tx.repo().store().get_root_tree(&tree_id)?;
    if middle_tree.id() == base_tree.id() {
        writeln!(
            ui.warning(),
            "The given paths do not match any file: {}",
            args.paths.join(" ")
        )?;
    }

    let first_template = description_template_for_cmd_split(
        ui,
        command.settings(),
        tx.base_workspace_helper(),
        "Enter commit description for the first part (parent).",
        commit.description(),
        &base_tree,
        &middle_tree,
    )?;
    let first_description = edit_description(tx.base_repo(), &first_template, command.settings())?;
    let first_commit = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_tree_id(tree_id)
        .set_description(first_description)
        .write()?;
    let second_description = if commit.description().is_empty() {
        // If there was no description before, don't ask for one for the second commit.
        "".to_string()
    } else {
        let second_template = description_template_for_cmd_split(
            ui,
            command.settings(),
            tx.base_workspace_helper(),
            "Enter commit description for the second part (child).",
            commit.description(),
            &middle_tree,
            &end_tree,
        )?;
        edit_description(tx.base_repo(), &second_template, command.settings())?
    };
    let second_commit = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_parents(vec![first_commit.id().clone()])
        .set_tree_id(commit.tree_id().clone())
        .generate_new_change_id()
        .set_description(second_description)
        .write()?;
    let mut rebaser = DescendantRebaser::new(
        command.settings(),
        tx.mut_repo(),
        hashmap! { commit.id().clone() => hashset!{second_commit.id().clone()} },
        hashset! {},
    );
    rebaser.rebase_all()?;
    let num_rebased = rebaser.rebased().len();
    if num_rebased > 0 {
        writeln!(ui.stderr(), "Rebased {num_rebased} descendant commits")?;
    }
    write!(ui.stderr(), "First part: ")?;
    tx.write_commit_summary(ui.stderr_formatter().as_mut(), &first_commit)?;
    write!(ui.stderr(), "\nSecond part: ")?;
    tx.write_commit_summary(ui.stderr_formatter().as_mut(), &second_commit)?;
    writeln!(ui.stderr())?;
    tx.finish(ui)?;
    Ok(())
}

fn description_template_for_cmd_split(
    ui: &Ui,
    settings: &UserSettings,
    workspace_command: &WorkspaceCommandHelper,
    intro: &str,
    overall_commit_description: &str,
    from_tree: &MergedTree,
    to_tree: &MergedTree,
) -> Result<String, CommandError> {
    let mut diff_summary_bytes = Vec::new();
    diff_util::show_diff(
        ui,
        &mut PlainTextFormatter::new(&mut diff_summary_bytes),
        workspace_command,
        from_tree,
        to_tree,
        &EverythingMatcher,
        &[DiffFormat::Summary],
    )?;
    let description = if overall_commit_description.is_empty() {
        settings.default_description()
    } else {
        overall_commit_description.to_owned()
    };
    Ok(format!("JJ: {intro}\n{description}\n") + &diff_summary_to_description(&diff_summary_bytes))
}
