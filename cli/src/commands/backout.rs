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
use jj_lib::rewrite::back_out_commit;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::CommandError;
use crate::commit_templater::CommitTemplateLanguage;
use crate::formatter::PlainTextFormatter;
use crate::ui::Ui;

/// Apply the reverse of a revision on top of another revision
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct BackoutArgs {
    /// The revision to apply the reverse of
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// The revision to apply the reverse changes on top of
    // TODO: It seems better to default this to `@-`. Maybe the working
    // copy should be rebased on top?
    #[arg(long, short, default_value = "@")]
    destination: Vec<RevisionArg>,
    /// Template used to generate the commit message
    ///
    /// For the syntax, see https://github.com/martinvonz/jj/blob/main/docs/templates.md
    #[arg(long, short = 'T')]
    template: Option<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_backout(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BackoutArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit_to_back_out = workspace_command.resolve_single_rev(&args.revision)?;
    let mut parents = vec![];
    for revision_str in &args.destination {
        let destination = workspace_command.resolve_single_rev(revision_str)?;
        parents.push(destination);
    }
    let mut tx = workspace_command.start_transaction();
    let commit_description = {
        let language = tx.base_workspace_helper().commit_template_language()?;
        let template_string = match &args.template {
            Some(value) => value.to_string(),
            None => command
                .settings()
                .config()
                .get_string("templates.backout")?,
        };
        let template = tx.base_workspace_helper().parse_template(
            &language,
            &template_string,
            CommitTemplateLanguage::wrap_commit,
        )?;
        let mut output = Vec::new();
        let mut formatter = PlainTextFormatter::new(&mut output);
        template.format(&commit_to_back_out, &mut formatter)?;
        String::from_utf8(output).expect("template output should be utf-8 bytes")
    };
    back_out_commit(
        command.settings(),
        tx.mut_repo(),
        &commit_to_back_out,
        &parents,
        Some(commit_description),
    )?;
    tx.finish(
        ui,
        format!("back out commit {}", commit_to_back_out.id().hex()),
    )?;

    Ok(())
}
