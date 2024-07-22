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
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::settings::UserSettings;
use tracing::instrument;

use crate::cli_util::{CommandHelper, WorkspaceCommandHelper};
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Display information for each line of file, showing the source change of each
/// line.
///
/// Annotates a revision line by line. Each line includes the source change that
/// committed the associated line. A path to the desired file must be provided
/// and this command will fail if the file is in a conflicted state currently or
/// in previous changes. The per line prefix for each line can be customized via
/// template with the `templates.annotate_commit_summary` config variable
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct AnnotateArgs {
    /// the file to annotate
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    path: String,
}

#[instrument(skip_all)]
pub(crate) fn cmd_annotate(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &AnnotateArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let starting_commit_id = workspace_command.get_wc_commit_id().unwrap();
    let repo = workspace_command.repo();
    let file_path = RepoPathBuf::from_relative_path(&args.path);
    if file_path.is_err() {
        eprintln!("Unable to locate file: {}", args.path);
        return Ok(());
    }

    let res = get_annotation_for_file(&file_path.unwrap(), repo, starting_commit_id);
    if let Err(e) = res {
        eprintln!("{}", e);
        return Ok(());
    }
    let res = res.unwrap();
    render_annotations(repo, ui, command.settings(), &workspace_command, &res)?;
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
