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

use jj_lib::rewrite::merge_commit_trees;
use tracing::instrument;

use crate::cli_util::{CommandError, CommandHelper, RevisionArg};
use crate::diff_util::{diff_formats_for, show_diff, DiffFormatArgs};
use crate::ui::Ui;

#[derive(clap::Args, Clone, Debug)]
pub(crate) struct DiffArgs {
    /// Show changes in this revision, compared to its parent(s)
    #[arg(long, short)]
    revision: Option<RevisionArg>,
    /// Show changes from this revision
    #[arg(long, conflicts_with = "revision")]
    from: Option<RevisionArg>,
    /// Show changes to this revision
    #[arg(long, conflicts_with = "revision")]
    to: Option<RevisionArg>,
    /// Restrict the diff to these paths
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
    #[command(flatten)]
    format: DiffFormatArgs,
}

#[instrument(skip_all)]
pub(crate) fn cmd_diff(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DiffArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let from_tree;
    let to_tree;
    if args.from.is_some() || args.to.is_some() {
        let from = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"), ui)?;
        from_tree = from.tree()?;
        let to = workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"), ui)?;
        to_tree = to.tree()?;
    } else {
        let commit =
            workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"), ui)?;
        let parents = commit.parents();
        from_tree = merge_commit_trees(workspace_command.repo().as_ref(), &parents)?;
        to_tree = commit.tree()?
    }
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let diff_formats = diff_formats_for(command.settings(), &args.format)?;
    ui.request_pager();
    show_diff(
        ui,
        ui.stdout_formatter().as_mut(),
        &workspace_command,
        &from_tree,
        &to_tree,
        matcher.as_ref(),
        &diff_formats,
    )?;
    Ok(())
}
