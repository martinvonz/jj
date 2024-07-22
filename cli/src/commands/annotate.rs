// Copyright 2024 The Jujutsu Authors
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

use jj_lib::annotate::{get_annotation_for_file, AnnotateResults};
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::settings::UserSettings;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg, WorkspaceCommandHelper};
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

/// Show the source change for each line of the target file.
///
/// Annotates a revision line by line. Each line includes the source change that
/// introduced the associated line. A path to the desired file must be provided.
/// This command will fail if the file is in a conflicted state currently or
/// in previous changes. The per-line prefix for each line can be customized via
/// template with the `templates.annotate_commit_summary` config variable.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct AnnotateArgs {
    /// the file to annotate
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    path: String,
    /// an optional revision to start at
    #[arg(long, short)]
    revision: Option<RevisionArg>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_annotate(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &AnnotateArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let starting_commit =
        workspace_command.resolve_single_rev(args.revision.as_ref().unwrap_or(&RevisionArg::AT))?;
    let file_path = workspace_command.parse_file_path(&args.path)?;
    let file_value = starting_commit.tree()?.path_value(&file_path)?;
    if file_value.is_absent() {
        let ui_path = workspace_command.format_file_path(&file_path);
        return Err(user_error(format!("No such path: {ui_path}")));
    }

    let annotations = get_annotation_for_file(&file_path, repo, &starting_commit);
    if let Err(e) = annotations {
        eprintln!("{}", e);
        return Ok(());
    }
    let annotations = annotations.unwrap();
    render_annotations(
        repo,
        ui,
        command.settings(),
        &workspace_command,
        &annotations,
    )?;
    Ok(())
}

fn render_annotations(
    repo: &ReadonlyRepo,
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &WorkspaceCommandHelper,
    results: &AnnotateResults,
) -> Result<(), CommandError> {
    let annotate_commit_summary_text = settings
        .config()
        .get_string("templates.annotate_commit_summary")?;
    let template = workspace_command.parse_commit_template(&annotate_commit_summary_text)?;
    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    for (line_no, (commit_id, line)) in results.file_annotations.iter().enumerate() {
        let commit = repo.store().get_commit(commit_id)?;
        template.format(&commit, formatter.as_mut())?;
        writeln!(formatter, " {}: {}", line_no + 1, line)?;
    }

    Ok(())
}
