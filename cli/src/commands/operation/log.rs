// Copyright 2020-2023 The Jujutsu Authors
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

use std::slice;

use itertools::Itertools as _;
use jj_lib::config::ConfigError;
use jj_lib::op_walk;
use jj_lib::operation::Operation;
use jj_lib::repo::RepoLoader;
use jj_lib::settings::ConfigResultExt as _;
use jj_lib::settings::UserSettings;

use super::diff::show_op_diff;
use crate::cli_util::format_template;
use crate::cli_util::CommandHelper;
use crate::cli_util::LogContentFormat;
use crate::cli_util::WorkspaceCommandEnvironment;
use crate::command_error::CommandError;
use crate::commit_templater::CommitTemplateLanguage;
use crate::diff_util::diff_formats_for_log;
use crate::diff_util::DiffFormatArgs;
use crate::diff_util::DiffRenderer;
use crate::formatter::Formatter;
use crate::graphlog::get_graphlog;
use crate::graphlog::Edge;
use crate::graphlog::GraphStyle;
use crate::operation_templater::OperationTemplateLanguage;
use crate::ui::Ui;

/// Show the operation log
///
/// Like other commands, `jj op log` snapshots the current working-copy changes
/// and reconciles divergent operations. Use `--at-op=@ --ignore-working-copy`
/// to inspect the current state without mutation.
#[derive(clap::Args, Clone, Debug)]
pub struct OperationLogArgs {
    /// Limit number of operations to show
    #[arg(long, short = 'n')]
    limit: Option<usize>,
    // TODO: Delete `-l` alias in jj 0.25+
    #[arg(
        short = 'l',
        hide = true,
        conflicts_with = "limit",
        value_name = "LIMIT"
    )]
    deprecated_limit: Option<usize>,
    /// Don't show the graph, show a flat list of operations
    #[arg(long)]
    no_graph: bool,
    /// Render each operation using the given template
    ///
    /// For the syntax, see https://martinvonz.github.io/jj/latest/templates/
    #[arg(long, short = 'T')]
    template: Option<String>,
    /// Show changes to the repository at each operation
    #[arg(long)]
    op_diff: bool,
    /// Show patch of modifications to changes (implies --op-diff)
    ///
    /// If the previous version has different parents, it will be temporarily
    /// rebased to the parents of the new version, so the diff is not
    /// contaminated by unrelated changes.
    #[arg(long, short = 'p')]
    patch: bool,
    #[command(flatten)]
    diff_format: DiffFormatArgs,
}

pub fn cmd_op_log(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationLogArgs,
) -> Result<(), CommandError> {
    if command.is_working_copy_writable() {
        let workspace_command = command.workspace_helper(ui)?;
        let current_op = workspace_command.repo().operation();
        let repo_loader = workspace_command.workspace().repo_loader();
        do_op_log(ui, workspace_command.env(), repo_loader, current_op, args)
    } else {
        // Don't load the repo so that the operation history can be inspected
        // even with a corrupted repo state. For example, you can find the first
        // bad operation id to be abandoned.
        let workspace = command.load_workspace()?;
        let workspace_env = command.workspace_environment(ui, &workspace)?;
        let repo_loader = workspace.repo_loader();
        let current_op = command.resolve_operation(ui, workspace.repo_loader())?;
        do_op_log(ui, &workspace_env, repo_loader, &current_op, args)
    }
}

fn do_op_log(
    ui: &mut Ui,
    workspace_env: &WorkspaceCommandEnvironment,
    repo_loader: &RepoLoader,
    current_op: &Operation,
    args: &OperationLogArgs,
) -> Result<(), CommandError> {
    let settings = workspace_env.settings();
    let graph_style = GraphStyle::from_settings(settings)?;
    let with_content_format = LogContentFormat::new(ui, settings)?;

    let template;
    let op_node_template;
    {
        let language = OperationTemplateLanguage::new(
            repo_loader.op_store().root_operation_id(),
            Some(current_op.id()),
            workspace_env.operation_template_extensions(),
        );
        let text = match &args.template {
            Some(value) => value.to_owned(),
            None => settings.get_string("templates.op_log")?,
        };
        template = workspace_env
            .parse_template(
                ui,
                &language,
                &text,
                OperationTemplateLanguage::wrap_operation,
            )?
            .labeled("operation")
            .labeled("op_log");
        op_node_template = workspace_env
            .parse_template(
                ui,
                &language,
                &get_node_template(graph_style, settings)?,
                OperationTemplateLanguage::wrap_operation,
            )?
            .labeled("node");
    }

    let diff_formats = diff_formats_for_log(settings, &args.diff_format, args.patch)?;
    let maybe_show_op_diff = if args.op_diff || !diff_formats.is_empty() {
        let template_text = settings.get_string("templates.commit_summary")?;
        let show = move |ui: &Ui,
                         formatter: &mut dyn Formatter,
                         op: &Operation,
                         with_content_format: &LogContentFormat| {
            let parents: Vec<_> = op.parents().try_collect()?;
            let parent_op = repo_loader.merge_operations(settings, parents, None)?;
            let parent_repo = repo_loader.load_at(&parent_op)?;
            let repo = repo_loader.load_at(op)?;

            let id_prefix_context = workspace_env.new_id_prefix_context();
            let commit_summary_template = {
                let language =
                    workspace_env.commit_template_language(repo.as_ref(), &id_prefix_context);
                workspace_env.parse_template(
                    ui,
                    &language,
                    &template_text,
                    CommitTemplateLanguage::wrap_commit,
                )?
            };
            let path_converter = workspace_env.path_converter();
            let conflict_marker_style = workspace_env.conflict_marker_style();
            let diff_renderer = (!diff_formats.is_empty()).then(|| {
                DiffRenderer::new(
                    repo.as_ref(),
                    path_converter,
                    conflict_marker_style,
                    diff_formats.clone(),
                )
            });

            show_op_diff(
                ui,
                formatter,
                repo.as_ref(),
                &parent_repo,
                &repo,
                &commit_summary_template,
                (!args.no_graph).then_some(graph_style),
                with_content_format,
                diff_renderer.as_ref(),
            )
        };
        Some(show)
    } else {
        None
    };

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();
    if args.deprecated_limit.is_some() {
        writeln!(
            ui.warning_default(),
            "The -l shorthand is deprecated, use -n instead."
        )?;
    }
    let limit = args.limit.or(args.deprecated_limit).unwrap_or(usize::MAX);
    let iter = op_walk::walk_ancestors(slice::from_ref(current_op)).take(limit);
    if !args.no_graph {
        let mut raw_output = formatter.raw()?;
        let mut graph = get_graphlog(graph_style, raw_output.as_mut());
        for op in iter {
            let op = op?;
            let mut edges = vec![];
            for id in op.parent_ids() {
                edges.push(Edge::Direct(id.clone()));
            }
            let mut buffer = vec![];
            let within_graph = with_content_format.sub_width(graph.width(op.id(), &edges));
            within_graph.write(ui.new_formatter(&mut buffer).as_mut(), |formatter| {
                template.format(&op, formatter)
            })?;
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
            }
            if let Some(show) = &maybe_show_op_diff {
                let mut formatter = ui.new_formatter(&mut buffer);
                show(ui, formatter.as_mut(), &op, &within_graph)?;
            }
            let node_symbol = format_template(ui, &op, &op_node_template);
            graph.add_node(
                op.id(),
                &edges,
                &node_symbol,
                &String::from_utf8_lossy(&buffer),
            )?;
        }
    } else {
        for op in iter {
            let op = op?;
            with_content_format.write(formatter, |formatter| template.format(&op, formatter))?;
            if let Some(show) = &maybe_show_op_diff {
                show(ui, formatter, &op, &with_content_format)?;
            }
        }
    }

    Ok(())
}

fn get_node_template(style: GraphStyle, settings: &UserSettings) -> Result<String, ConfigError> {
    let symbol = settings.get_string("templates.op_log_node").optional()?;
    let default = if style.is_ascii() {
        "builtin_op_log_node_ascii"
    } else {
        "builtin_op_log_node"
    };
    Ok(symbol.unwrap_or_else(|| default.to_owned()))
}
