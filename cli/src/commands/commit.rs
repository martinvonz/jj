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
use jj_lib::repo::Repo;
use jj_lib::rewrite::merge_commit_trees;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error::{user_error, CommandError};
use crate::description_util::{
    description_template_for_commit, edit_description, join_message_paragraphs,
};
use crate::ui::Ui;

/// Update the description and create a new change on top.
#[derive(clap::Args, Clone, Debug)]
#[command(visible_aliases=&["ci"])]
pub(crate) struct CommitArgs {
    /// Interactively choose which changes to include in the first commit
    #[arg(short, long)]
    interactive: bool,
    /// Specify diff editor to be used (implies --interactive)
    #[arg(long, value_name = "NAME")]
    tool: Option<String>,
    /// The change description to use (don't open editor)
    #[arg(long = "message", short, value_name = "MESSAGE")]
    message_paragraphs: Vec<String>,
    /// Put these paths in the first commit
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_commit(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &CommitArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let commit_id = workspace_command
        .get_wc_commit_id()
        .ok_or_else(|| user_error("This command requires a working copy"))?;
    let commit = workspace_command.repo().store().get_commit(commit_id)?;
    let matcher = workspace_command
        .parse_file_patterns(&args.paths)?
        .to_matcher();
    let advanceable_branches = workspace_command.get_advanceable_branches(commit.parent_ids())?;
    let diff_selector =
        workspace_command.diff_selector(ui, args.tool.as_deref(), args.interactive)?;
    let mut tx = workspace_command.start_transaction();
    let base_tree = merge_commit_trees(tx.repo(), &commit.parents())?;
    let instructions = format!(
        "\
You are splitting the working-copy commit: {}

The diff initially shows all changes. Adjust the right side until it shows the
contents you want for the first commit. The remainder will be included in the
new working-copy commit.
",
        tx.format_commit_summary(&commit)
    );
    let tree_id = diff_selector.select(
        &base_tree,
        &commit.tree()?,
        matcher.as_ref(),
        Some(&instructions),
    )?;
    let middle_tree = tx.repo().store().get_root_tree(&tree_id)?;
    if !args.paths.is_empty() && middle_tree.id() == base_tree.id() {
        writeln!(
            ui.warning_default(),
            "The given paths do not match any file: {}",
            args.paths.join(" ")
        )?;
    }

    let template = description_template_for_commit(
        ui,
        command.settings(),
        tx.base_workspace_helper(),
        "",
        commit.description(),
        &base_tree,
        &middle_tree,
    )?;

    let description = if !args.message_paragraphs.is_empty() {
        join_message_paragraphs(&args.message_paragraphs)
    } else {
        edit_description(tx.base_repo(), &template, command.settings())?
    };

    let new_commit = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_tree_id(tree_id)
        .set_description(description)
        .write()?;
    let workspace_ids = tx
        .mut_repo()
        .view()
        .workspaces_for_wc_commit_id(commit.id());
    if !workspace_ids.is_empty() {
        let new_wc_commit = tx
            .mut_repo()
            .new_commit(
                command.settings(),
                vec![new_commit.id().clone()],
                commit.tree_id().clone(),
            )
            .write()?;

        // Does nothing if there's no branches to advance.
        tx.advance_branches(ui, advanceable_branches, new_commit.id());

        for workspace_id in workspace_ids {
            tx.mut_repo().edit(workspace_id, &new_wc_commit).unwrap();
        }
    }
    tx.finish(ui, format!("commit {}", commit.id().hex()))?;
    Ok(())
}
