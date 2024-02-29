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

use clap::parser::ValueSource;
use jj_lib::object_id::ObjectId;
use jj_lib::revset;
use tracing::instrument;

use crate::cli_util::{self, user_error, CommandError, CommandHelper, RevisionArg};
use crate::description_util::combine_messages;
use crate::ui::Ui;

/// Move changes from a revision into its parent
///
/// After moving the changes into the parent, the child revision will have the
/// same content state as before. If that means that the change is now empty
/// compared to its parent, it will be abandoned.
/// Without `--interactive`, the child change will always be empty.
///
/// If the source became empty and both the source and destination had a
/// non-empty description, you will be asked for the combined description. If
/// either was empty, then the other one will be used.
///
/// If a working-copy commit gets abandoned, it will be given a new, empty
/// commit. This is true in general; it is not specific to this command.
#[derive(clap::Args, Clone, Debug)]
#[command(visible_alias = "amend")]
pub(crate) struct SquashArgs {
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// The description to use for squashed revision (don't open editor)
    #[arg(long = "message", short, value_name = "MESSAGE")]
    message_paragraphs: Vec<String>,
    /// Interactively choose which parts to squash
    #[arg(long, short)]
    interactive: bool,
    /// Move only changes to these paths (instead of all paths)
    #[arg(conflicts_with = "interactive", value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_squash(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SquashArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewritable([&commit])?;
    let parents = commit.parents();
    if parents.len() != 1 {
        return Err(user_error("Cannot squash merge commits"));
    }
    let parent = &parents[0];
    workspace_command.check_rewritable(&parents[..1])?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let mut tx = workspace_command.start_transaction();
    let instructions = format!(
        "\
You are moving changes from: {}
into its parent: {}

The left side of the diff shows the contents of the parent commit. The
right side initially shows the contents of the commit you're moving
changes from.

Adjust the right side until the diff shows the changes you want to move
to the destination. If you don't make any changes, then all the changes
from the source will be moved into the parent.
",
        tx.format_commit_summary(&commit),
        tx.format_commit_summary(parent)
    );
    let parent_tree = parent.tree()?;
    let tree = commit.tree()?;
    let new_parent_tree_id = tx.select_diff(
        ui,
        &parent_tree,
        &tree,
        matcher.as_ref(),
        Some(&instructions),
        args.interactive,
    )?;
    if &new_parent_tree_id == parent.tree_id() {
        if args.interactive {
            return Err(user_error("No changes selected"));
        }

        if let [only_path] = &args.paths[..] {
            let (_, matches) = command.matches().subcommand().unwrap();
            if matches.value_source("revision").unwrap() == ValueSource::DefaultValue
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
    // Abandon the child if the parent now has all the content from the child
    // (always the case in the non-interactive case).
    let abandon_child = &new_parent_tree_id == commit.tree_id();
    let description = if !args.message_paragraphs.is_empty() {
        cli_util::join_message_paragraphs(&args.message_paragraphs)
    } else {
        combine_messages(
            tx.base_repo(),
            &commit,
            parent,
            command.settings(),
            abandon_child,
        )?
    };
    let mut_repo = tx.mut_repo();
    let new_parent = mut_repo
        .rewrite_commit(command.settings(), parent)
        .set_tree_id(new_parent_tree_id)
        .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
        .set_description(description)
        .write()?;
    if abandon_child {
        mut_repo.record_abandoned_commit(commit.id().clone());
    } else {
        // Commit the remainder on top of the new parent commit.
        mut_repo
            .rewrite_commit(command.settings(), &commit)
            .set_parents(vec![new_parent.id().clone()])
            .write()?;
    }
    tx.finish(ui, format!("squash commit {}", commit.id().hex()))?;
    Ok(())
}
