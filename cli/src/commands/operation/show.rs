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

use itertools::Itertools;

use super::diff::show_op_diff;
use crate::cli_util::CommandHelper;
use crate::cli_util::LogContentFormat;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::diff_util::DiffFormatArgs;
use crate::graphlog::GraphStyle;
use crate::operation_templater::OperationTemplateLanguage;
use crate::ui::Ui;

/// Show changes to the repository in an operation
#[derive(clap::Args, Clone, Debug)]
pub struct OperationShowArgs {
    /// Show repository changes in this operation, compared to its parent(s)
    #[arg(default_value = "@")]
    operation: String,
    /// Don't show the graph, show a flat list of modified changes
    #[arg(long)]
    no_graph: bool,
    /// Show patch of modifications to changes
    ///
    /// If the previous version has different parents, it will be temporarily
    /// rebased to the parents of the new version, so the diff is not
    /// contaminated by unrelated changes.
    #[arg(long, short = 'p')]
    patch: bool,
    #[command(flatten)]
    diff_format: DiffFormatArgs,
}

pub fn cmd_op_show(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationShowArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let current_op_id = repo.operation().id();
    let repo_loader = &repo.loader();
    let op = workspace_command.resolve_single_op(&args.operation)?;
    let parents: Vec<_> = op.parents().try_collect()?;
    if parents.is_empty() {
        return Err(user_error("Cannot show the root operation"));
    }
    let parent_op = repo_loader.merge_operations(command.settings(), parents, None)?;
    let parent_repo = repo_loader.load_at(&parent_op)?;
    let repo = repo_loader.load_at(&op)?;

    let workspace_command =
        command.for_temporary_repo(ui, command.load_workspace()?, repo.clone())?;
    let commit_summary_template = workspace_command.commit_summary_template();

    let graph_style = GraphStyle::from_settings(command.settings())?;
    let with_content_format = LogContentFormat::new(ui, command.settings())?;
    let diff_renderer = workspace_command.diff_renderer_for_log(&args.diff_format, args.patch)?;

    // TODO: Should we make this customizable via clap arg?
    let template;
    {
        let language = OperationTemplateLanguage::new(
            repo_loader.op_store().root_operation_id(),
            Some(current_op_id),
            command.operation_template_extensions(),
        );
        let text = command.settings().config().get_string("templates.op_log")?;
        template = workspace_command
            .parse_template(&language, &text, OperationTemplateLanguage::wrap_operation)?
            .labeled("op_log");
    }

    ui.request_pager();
    template.format(&op, ui.stdout_formatter().as_mut())?;

    show_op_diff(
        ui,
        repo.as_ref(),
        &parent_repo,
        &repo,
        &commit_summary_template,
        (!args.no_graph).then_some(graph_style),
        &with_content_format,
        diff_renderer,
    )
}
