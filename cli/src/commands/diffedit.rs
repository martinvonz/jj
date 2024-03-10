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

use jj_lib::matchers::EverythingMatcher;
use jj_lib::object_id::ObjectId;
use jj_lib::rewrite::merge_commit_trees;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Touch up the content changes in a revision with a diff editor
///
/// With the `-r` option, which is the default, starts a diff editor (`meld` by
/// default) on the changes in the revision.
///
/// With the `--from` and/or `--to` options, starts a diff editor comparing the
/// "from" revision to the "to" revision.
///
/// Edit the right side of the diff until it looks the way you want. Once you
/// close the editor, the revision specified with `-r` or `--to` will be
/// updated. Descendants will be rebased on top as usual, which may result in
/// conflicts.
///
/// See `jj restore` if you want to move entire files from one revision to
/// another. See `jj squash -i` or `jj unsquash -i` if you instead want to move
/// changes into or out of the parent revision.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct DiffeditArgs {
    /// The revision to touch up. Defaults to @ if neither --to nor --from are
    /// specified.
    #[arg(long, short)]
    revision: Option<RevisionArg>,
    /// Show changes from this revision. Defaults to @ if --to is specified.
    #[arg(long, conflicts_with = "revision")]
    from: Option<RevisionArg>,
    /// Edit changes in this revision. Defaults to @ if --from is specified.
    #[arg(long, conflicts_with = "revision")]
    to: Option<RevisionArg>,
    /// Specify diff editor to be used
    #[arg(long, value_name = "NAME")]
    tool: Option<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_diffedit(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DiffeditArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let (target_commit, base_commits, diff_description);
    if args.from.is_some() || args.to.is_some() {
        target_commit = workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"))?;
        base_commits =
            vec![workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"))?];
        diff_description = format!(
            "The diff initially shows the commit's changes relative to:\n{}",
            workspace_command.format_commit_summary(&base_commits[0])
        );
    } else {
        target_commit =
            workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"))?;
        base_commits = target_commit.parents();
        diff_description = "The diff initially shows the commit's changes.".to_string();
    };
    workspace_command.check_rewritable([&target_commit])?;

    let diff_editor = workspace_command.diff_editor(ui, args.tool.as_deref())?;
    let mut tx = workspace_command.start_transaction();
    let instructions = format!(
        "\
You are editing changes in: {}

{diff_description}

Adjust the right side until it shows the contents you want. If you
don't make any changes, then the operation will be aborted.",
        tx.format_commit_summary(&target_commit),
    );
    let base_tree = merge_commit_trees(tx.repo(), base_commits.as_slice())?;
    let tree = target_commit.tree()?;
    let tree_id = diff_editor.edit(&base_tree, &tree, &EverythingMatcher, Some(&instructions))?;
    if tree_id == *target_commit.tree_id() {
        writeln!(ui.stderr(), "Nothing changed.")?;
    } else {
        let mut_repo = tx.mut_repo();
        let new_commit = mut_repo
            .rewrite_commit(command.settings(), &target_commit)
            .set_tree_id(tree_id)
            .write()?;
        // rebase_descendants early; otherwise `new_commit` would always have
        // a conflicted change id at this point.
        let num_rebased = tx.mut_repo().rebase_descendants(command.settings())?;
        write!(ui.stderr(), "Created ")?;
        tx.write_commit_summary(ui.stderr_formatter().as_mut(), &new_commit)?;
        writeln!(ui.stderr())?;
        if num_rebased > 0 {
            writeln!(ui.stderr(), "Rebased {num_rebased} descendant commits")?;
        }
        tx.finish(ui, format!("edit commit {}", target_commit.id().hex()))?;
    }
    Ok(())
}
