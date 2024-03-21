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

use jj_lib::matchers::EverythingMatcher;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::CommandError;
use crate::diff_util::{self, DiffFormatArgs};
use crate::ui::Ui;

/// Show commit description and changes in a revision
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct ShowArgs {
    /// Show changes in this revision, compared to its parent(s)
    #[arg(default_value = "@")]
    revision: RevisionArg,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
    /// Render a revision using the given template
    ///
    /// For the syntax, see https://github.com/martinvonz/jj/blob/main/docs/templates.md
    #[arg(long, short = 'T')]
    template: Option<String>,
    #[command(flatten)]
    format: DiffFormatArgs,
}

#[instrument(skip_all)]
pub(crate) fn cmd_show(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ShowArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    let template_string = match &args.template {
        Some(value) => value.to_string(),
        None => command.settings().config().get_string("templates.show")?,
    };
    let template = workspace_command.parse_commit_template(&template_string)?;
    let diff_formats = diff_util::diff_formats_for(command.settings(), &args.format)?;
    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();
    template.format(&commit, formatter)?;
    diff_util::show_patch(
        ui,
        formatter,
        &workspace_command,
        &commit,
        &EverythingMatcher,
        &diff_formats,
    )?;
    Ok(())
}
