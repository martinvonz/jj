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

use jj_lib::op_walk;

use crate::cli_util::{format_template, CommandHelper, LogContentFormat};
use crate::command_error::CommandError;
use crate::graphlog::{get_graphlog, Edge};
use crate::operation_templater::OperationTemplateLanguage;
use crate::ui::Ui;

/// Show the operation log
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
    /// For the syntax, see https://github.com/martinvonz/jj/blob/main/docs/templates.md
    #[arg(long, short = 'T')]
    template: Option<String>,
}

pub fn cmd_op_log(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationLogArgs,
) -> Result<(), CommandError> {
    // Don't load the repo so that the operation history can be inspected even
    // with a corrupted repo state. For example, you can find the first bad
    // operation id to be abandoned.
    let workspace = command.load_workspace()?;
    let repo_loader = workspace.repo_loader();
    let head_op_str = &command.global_args().at_operation;
    let head_ops = if head_op_str == "@" {
        // If multiple head ops can't be resolved without merging, let the
        // current op be empty. Beware that resolve_op_for_load() will eliminate
        // redundant heads whereas get_current_head_ops() won't.
        let current_op = op_walk::resolve_op_for_load(repo_loader, head_op_str).ok();
        if let Some(op) = current_op {
            vec![op]
        } else {
            op_walk::get_current_head_ops(
                repo_loader.op_store(),
                repo_loader.op_heads_store().as_ref(),
            )?
        }
    } else {
        vec![op_walk::resolve_op_for_load(repo_loader, head_op_str)?]
    };
    let current_op_id = match &*head_ops {
        [op] => Some(op.id()),
        _ => None,
    };
    let with_content_format = LogContentFormat::new(ui, command.settings())?;

    let template;
    let op_node_template;
    {
        let language = OperationTemplateLanguage::new(
            repo_loader.op_store().root_operation_id(),
            current_op_id,
            command.operation_template_extensions(),
        );
        let text = match &args.template {
            Some(value) => value.to_owned(),
            None => command.settings().config().get_string("templates.op_log")?,
        };
        template = command
            .parse_template(
                ui,
                &language,
                &text,
                OperationTemplateLanguage::wrap_operation,
            )?
            .labeled("op_log");
        op_node_template = command
            .parse_template(
                ui,
                &language,
                &command.settings().op_node_template(),
                OperationTemplateLanguage::wrap_operation,
            )?
            .labeled("node");
    }

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
    let iter = op_walk::walk_ancestors(&head_ops).take(limit);
    if !args.no_graph {
        let mut graph = get_graphlog(command.settings(), formatter.raw());
        for op in iter {
            let op = op?;
            let mut edges = vec![];
            for id in op.parent_ids() {
                edges.push(Edge::Direct(id.clone()));
            }
            let mut buffer = vec![];
            with_content_format.write_graph_text(
                ui.new_formatter(&mut buffer).as_mut(),
                |formatter| template.format(&op, formatter),
                || graph.width(op.id(), &edges),
            )?;
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
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
        }
    }

    Ok(())
}
