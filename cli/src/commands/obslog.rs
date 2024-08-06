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

use itertools::Itertools;
use jj_lib::commit::Commit;
use jj_lib::dag_walk::topo_order_reverse_ok;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::repo::Repo;
use jj_lib::rewrite::rebase_to_dest_parent;
use tracing::instrument;

use crate::cli_util::{format_template, CommandHelper, LogContentFormat, RevisionArg};
use crate::command_error::CommandError;
use crate::commit_templater::CommitTemplateLanguage;
use crate::diff_util::{DiffFormatArgs, DiffRenderer};
use crate::formatter::Formatter;
use crate::graphlog::{get_graphlog, Edge};
use crate::ui::Ui;

/// Show how a change has evolved over time
///
/// Lists the previous commits which a change has pointed to. The current commit
/// of a change evolves when the change is updated, rebased, etc.
///
/// Name is derived from Merciual's obsolescence markers.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct ObslogArgs {
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Limit number of revisions to show
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
    let repo = workspace_command.repo().as_ref();

    let start_commit = workspace_command.resolve_single_rev(&args.revision)?;

    let diff_renderer = workspace_command.diff_renderer_for_log(&args.diff_format, args.patch)?;
    let with_content_format = LogContentFormat::new(ui, command.settings())?;

    let template;
    let node_template;
    {
        let language = workspace_command.commit_template_language()?;
        let template_string = match &args.template {
            Some(value) => value.to_string(),
            None => command.settings().config().get_string("templates.log")?,
        };
        template = workspace_command
            .parse_template(
                &language,
                &template_string,
                CommitTemplateLanguage::wrap_commit,
            )?
            .labeled("log");
        node_template = workspace_command
            .parse_template(
                &language,
                &command.settings().commit_node_template(),
                CommitTemplateLanguage::wrap_commit_opt,
            )?
            .labeled("node");
    }

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();

    let mut commits = topo_order_reverse_ok(
        vec![Ok(start_commit)],
        |commit: &Commit| commit.id().clone(),
        |commit: &Commit| {
            let mut predecessors = commit.predecessors().collect_vec();
            // Predecessors don't need to follow any defined order. However in
            // practice, if there are multiple predecessors, then usually the
            // first predecessor is the previous version of the same change, and
            // the other predecessors are commits that were squashed into it. If
            // multiple commits are squashed at once, then they are usually
            // recorded in chronological order. We want to show squashed commits
            // in reverse chronological order, and we also want to show squashed
            // commits before the squash destination (since the destination's
            // subgraph may contain earlier squashed commits as well), so we
            // visit the predecessors in reverse order.
            predecessors.reverse();
            predecessors
        },
    )?;
    if args.deprecated_limit.is_some() {
        writeln!(
            ui.warning_default(),
            "The -l shorthand is deprecated, use -n instead."
        )?;
    }
    if let Some(n) = args.limit.or(args.deprecated_limit) {
        commits.truncate(n);
    }
    if !args.no_graph {
        let mut graph = get_graphlog(command.settings(), formatter.raw());
        for commit in commits {
            let mut edges = vec![];
            for predecessor in commit.predecessors() {
                edges.push(Edge::Direct(predecessor?.id().clone()));
            }
            let graph_width = || graph.width(commit.id(), &edges);
            let mut buffer = vec![];
            with_content_format.write_graph_text(
                ui.new_formatter(&mut buffer).as_mut(),
                |formatter| template.format(&commit, formatter),
                graph_width,
            )?;
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
            }
            if let Some(renderer) = &diff_renderer {
                let mut formatter = ui.new_formatter(&mut buffer);
                let width = usize::saturating_sub(ui.term_width(), graph_width());
                show_predecessor_patch(ui, repo, renderer, formatter.as_mut(), &commit, width)?;
            }
            let node_symbol = format_template(ui, &Some(commit.clone()), &node_template);
            graph.add_node(
                commit.id(),
                &edges,
                &node_symbol,
                &String::from_utf8_lossy(&buffer),
            )?;
        }
    } else {
        for commit in commits {
            with_content_format
                .write(formatter, |formatter| template.format(&commit, formatter))?;
            if let Some(renderer) = &diff_renderer {
                let width = ui.term_width();
                show_predecessor_patch(ui, repo, renderer, formatter, &commit, width)?;
            }
        }
    }

    Ok(())
}

fn show_predecessor_patch(
    ui: &Ui,
    repo: &dyn Repo,
    renderer: &DiffRenderer,
    formatter: &mut dyn Formatter,
    commit: &Commit,
    width: usize,
) -> Result<(), CommandError> {
    let predecessors: Vec<_> = commit.predecessors().try_collect()?;
    let predecessor_tree = rebase_to_dest_parent(repo, &predecessors, commit)?;
    let tree = commit.tree()?;
    renderer.show_diff(
        ui,
        formatter,
        &predecessor_tree,
        &tree,
        &EverythingMatcher,
        width,
    )?;
    Ok(())
}
