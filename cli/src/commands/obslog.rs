// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use jj_lib::commit::Commit;
use jj_lib::dag_walk::topo_order_reverse;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::rewrite::rebase_to_dest_parent;
use tracing::instrument;

use crate::cli_util::{
    CommandError, CommandHelper, LogContentFormat, RevisionArg, WorkspaceCommandHelper,
};
use crate::diff_util::{self, DiffFormat, DiffFormatArgs};
use crate::formatter::Formatter;
use crate::graphlog::{get_graphlog, Edge};
use crate::ui::Ui;

/// Show how a change has evolved
///
/// Show how a change has evolved as it's been updated, rebased, etc.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct ObslogArgs {
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Limit number of revisions to show
    #[arg(long, short)]
    limit: Option<usize>,
    /// Don't show the graph, show a flat list of revisions
    #[arg(long)]
    no_graph: bool,
    /// Render each revision using the given template
    ///
    /// For the syntax, see https://github.com/martinvonz/jj/blob/main/docs/templates.md
    #[arg(long, short = 'T')]
    template: Option<String>,
    /// Show patch compared to the previous version of this change
    ///
    /// If the previous version has different parents, it will be temporarily
    /// rebased to the parents of the new version, so the diff is not
    /// contaminated by unrelated changes.
    #[arg(long, short = 'p')]
    patch: bool,
    #[command(flatten)]
    diff_format: DiffFormatArgs,
}

#[instrument(skip_all)]
pub(crate) fn cmd_obslog(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ObslogArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;

    let start_commit = workspace_command.resolve_single_rev(&args.revision)?;
    let wc_commit_id = workspace_command.get_wc_commit_id();

    let diff_formats =
        diff_util::diff_formats_for_log(command.settings(), &args.diff_format, args.patch)?;

    let template_string = match &args.template {
        Some(value) => value.to_string(),
        None => command.settings().config().get_string("templates.log")?,
    };
    let template = workspace_command.parse_commit_template(&template_string)?;
    let with_content_format = LogContentFormat::new(ui, command.settings())?;

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();
    formatter.push_label("log")?;

    let mut commits = topo_order_reverse(
        vec![start_commit],
        |commit: &Commit| commit.id().clone(),
        |commit: &Commit| commit.predecessors(),
    );
    if let Some(n) = args.limit {
        commits.truncate(n);
    }
    if !args.no_graph {
        let mut graph = get_graphlog(command.settings(), formatter.raw());
        let default_node_symbol = graph.default_node_symbol().to_owned();
        for commit in commits {
            let mut edges = vec![];
            for predecessor in &commit.predecessors() {
                edges.push(Edge::direct(predecessor.id().clone()));
            }
            let mut buffer = vec![];
            with_content_format.write_graph_text(
                ui.new_formatter(&mut buffer).as_mut(),
                |formatter| template.format(&commit, formatter),
                || graph.width(commit.id(), &edges),
            )?;
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
            }
            if !diff_formats.is_empty() {
                let mut formatter = ui.new_formatter(&mut buffer);
                show_predecessor_patch(
                    ui,
                    formatter.as_mut(),
                    &workspace_command,
                    &commit,
                    &diff_formats,
                )?;
            }
            let node_symbol = if Some(commit.id()) == wc_commit_id {
                "@"
            } else {
                &default_node_symbol
            };
            graph.add_node(
                commit.id(),
                &edges,
                node_symbol,
                &String::from_utf8_lossy(&buffer),
            )?;
        }
    } else {
        for commit in commits {
            with_content_format
                .write(formatter, |formatter| template.format(&commit, formatter))?;
            if !diff_formats.is_empty() {
                show_predecessor_patch(ui, formatter, &workspace_command, &commit, &diff_formats)?;
            }
        }
    }

    Ok(())
}

fn show_predecessor_patch(
    ui: &Ui,
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
    diff_formats: &[DiffFormat],
) -> Result<(), CommandError> {
    let predecessors = commit.predecessors();
    let predecessor = match predecessors.first() {
        Some(predecessor) => predecessor,
        None => return Ok(()),
    };
    let predecessor_tree =
        rebase_to_dest_parent(workspace_command.repo().as_ref(), predecessor, commit)?;
    let tree = commit.tree()?;
    diff_util::show_diff(
        ui,
        formatter,
        workspace_command,
        &predecessor_tree,
        &tree,
        &EverythingMatcher,
        diff_formats,
    )
}
