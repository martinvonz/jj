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
use jj_lib::rewrite::rebase_to_dest_parent;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::CommandError;
use crate::diff_util::{self, DiffFormatArgs};
use crate::ui::Ui;

/// Compare the changes of two commits
///
/// This excludes changes from other commits by temporarily rebasing `--from`
/// onto `--to`'s parents. If you wish to compare the same change across
/// versions, consider `jj obslog -p` instead.
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("to_diff").args(&["from", "to"]).multiple(true).required(true)))]
pub(crate) struct InterdiffArgs {
    /// Show changes from this revision
    #[arg(long)]
    from: Option<RevisionArg>,
    /// Show changes to this revision
    #[arg(long)]
    to: Option<RevisionArg>,
    /// Restrict the diff to these paths
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
    #[command(flatten)]
    format: DiffFormatArgs,
}

#[instrument(skip_all)]
pub(crate) fn cmd_interdiff(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &InterdiffArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let from = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"))?;
    let to = workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"))?;

    let from_tree = rebase_to_dest_parent(workspace_command.repo().as_ref(), &from, &to)?;
    let to_tree = to.tree()?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let diff_formats = diff_util::diff_formats_for(command.settings(), &args.format)?;
    ui.request_pager();
    diff_util::show_diff(
        ui,
        ui.stdout_formatter().as_mut(),
        &workspace_command,
        &from_tree,
        &to_tree,
        matcher.as_ref(),
        &diff_formats,
    )
}
