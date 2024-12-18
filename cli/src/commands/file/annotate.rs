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

use clap_complete::ArgValueCandidates;
use clap_complete::ArgValueCompleter;
use jj_lib::annotate::get_annotation_for_file;
use jj_lib::annotate::FileAnnotation;
use jj_lib::commit::Commit;
use jj_lib::repo::Repo;
use jj_lib::revset::RevsetExpression;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::complete;
use crate::templater::TemplateRenderer;
use crate::ui::Ui;

/// Show the source change for each line of the target file.
///
/// Annotates a revision line by line. Each line includes the source change that
/// introduced the associated line. A path to the desired file must be provided.
/// The per-line prefix for each line can be customized via
/// template with the `templates.annotate_commit_summary` config variable.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct FileAnnotateArgs {
    /// the file to annotate
    #[arg(
        value_hint = clap::ValueHint::AnyPath,
        add = ArgValueCompleter::new(complete::all_revision_files),
    )]
    path: String,
    /// an optional revision to start at
    #[arg(
        long,
        short,
        value_name = "REVSET",
        add = ArgValueCandidates::new(complete::all_revisions)
    )]
    revision: Option<RevisionArg>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_file_annotate(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &FileAnnotateArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let starting_commit = workspace_command
        .resolve_single_rev(ui, args.revision.as_ref().unwrap_or(&RevisionArg::AT))?;
    let file_path = workspace_command.parse_file_path(&args.path)?;
    let file_value = starting_commit.tree()?.path_value(&file_path)?;
    let ui_path = workspace_command.format_file_path(&file_path);
    if file_value.is_absent() {
        return Err(user_error(format!("No such path: {ui_path}")));
    }
    if file_value.is_tree() {
        return Err(user_error(format!(
            "Path exists but is not a regular file: {ui_path}"
        )));
    }

    let annotate_commit_summary_text = command
        .settings()
        .get_string("templates.annotate_commit_summary")?;
    let template = workspace_command.parse_commit_template(ui, &annotate_commit_summary_text)?;

    // TODO: Should we add an option to limit the domain to e.g. recent commits?
    // Note that this is probably different from "--skip REVS", which won't
    // exclude the revisions, but will ignore diffs in those revisions as if
    // ancestor revisions had new content.
    let domain = RevsetExpression::all();
    let annotation = get_annotation_for_file(repo.as_ref(), &starting_commit, &domain, &file_path)?;

    render_file_annotation(repo.as_ref(), ui, &template, &annotation)?;
    Ok(())
}

fn render_file_annotation(
    repo: &dyn Repo,
    ui: &mut Ui,
    template_render: &TemplateRenderer<Commit>,
    annotation: &FileAnnotation,
) -> Result<(), CommandError> {
    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    for (line_no, (commit_id, line)) in annotation.lines().enumerate() {
        let commit_id = commit_id.expect("should reached to the empty ancestor");
        let commit = repo.store().get_commit(commit_id)?;
        template_render.format(&commit, formatter.as_mut())?;
        write!(formatter, " {:>4}: ", line_no + 1)?;
        formatter.write_all(line)?;
    }

    Ok(())
}
