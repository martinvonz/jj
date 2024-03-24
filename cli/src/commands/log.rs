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
use jj_lib::backend::CommitId;
use jj_lib::repo::Repo;
use jj_lib::revset::{self, RevsetExpression, RevsetFilterPredicate, RevsetIteratorExt};
use jj_lib::revset_graph::{
    ReverseRevsetGraphIterator, RevsetGraphEdgeType, TopoGroupedRevsetGraphIterator,
};
use tracing::instrument;

use crate::cli_util::{format_template, CommandHelper, LogContentFormat, RevisionArg};
use crate::command_error::CommandError;
use crate::commit_templater::CommitTemplateLanguage;
use crate::diff_util::{self, DiffFormatArgs};
use crate::graphlog::{get_graphlog, Edge};
use crate::ui::Ui;

/// Show revision history
///
/// Renders a graphical view of the project's history, ordered with children
/// before parents. By default, the output only includes mutable revisions,
/// along with some additional revisions for context.
///
/// Spans of revisions that are not included in the graph per `--revisions` are
/// rendered as a synthetic node labeled "(elided revisions)".
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct LogArgs {
    /// Which revisions to show. Defaults to the `revsets.log` setting, or
    /// `@ | ancestors(immutable_heads().., 2) | trunk()` if it is not set.
    #[arg(long, short)]
    revisions: Vec<RevisionArg>,
    /// Show revisions modifying the given paths
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
    /// Show revisions in the opposite order (older revisions first)
    #[arg(long)]
    reversed: bool,
    /// Limit number of revisions to show
    ///
    /// Applied after revisions are filtered and reordered.
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
    /// Show patch
    #[arg(long, short = 'p')]
    patch: bool,
    #[command(flatten)]
    diff_format: DiffFormatArgs,
}

#[instrument(skip_all)]
pub(crate) fn cmd_log(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &LogArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;

    let revset_expression = {
        let mut expression = if args.revisions.is_empty() {
            workspace_command.parse_revset(&command.settings().default_revset())?
        } else {
            workspace_command.parse_union_revsets(&args.revisions)?
        };
        if !args.paths.is_empty() {
            let repo_paths: Vec<_> = args
                .paths
                .iter()
                .map(|path_arg| workspace_command.parse_file_path(path_arg))
                .try_collect()?;
            expression = expression.intersection(&RevsetExpression::filter(
                RevsetFilterPredicate::File(Some(repo_paths)),
            ));
        }
        expression
    };
    let repo = workspace_command.repo();
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let revset = workspace_command.evaluate_revset(revset_expression)?;

    let store = repo.store();
    let diff_formats =
        diff_util::diff_formats_for_log(command.settings(), &args.diff_format, args.patch)?;

    let use_elided_nodes = command
        .settings()
        .config()
        .get_bool("ui.log-synthetic-elided-nodes")?;
    let with_content_format = LogContentFormat::new(ui, command.settings())?;

    let template;
    let node_template;
    {
        let language = workspace_command.commit_template_language()?;
        let template_string = match &args.template {
            Some(value) => value.to_string(),
            None => command.settings().config().get_string("templates.log")?,
        };
        template = workspace_command.parse_template(
            &language,
            &template_string,
            CommitTemplateLanguage::wrap_commit,
        )?;
        node_template = workspace_command.parse_template(
            &language,
            &command.settings().commit_node_template(),
            CommitTemplateLanguage::wrap_commit_opt,
        )?;
    }

    {
        ui.request_pager();
        let mut formatter = ui.stdout_formatter();
        let formatter = formatter.as_mut();

        if !args.no_graph {
            let mut graph = get_graphlog(command.settings(), formatter.raw());
            let forward_iter = TopoGroupedRevsetGraphIterator::new(revset.iter_graph());
            let iter: Box<dyn Iterator<Item = _>> = if args.reversed {
                Box::new(ReverseRevsetGraphIterator::new(forward_iter))
            } else {
                Box::new(forward_iter)
            };
            for (commit_id, edges) in iter.take(args.limit.unwrap_or(usize::MAX)) {
                // The graph is keyed by (CommitId, is_synthetic)
                let mut graphlog_edges = vec![];
                // TODO: Should we update revset.iter_graph() to yield this flag instead of all
                // the missing edges since we don't care about where they point here
                // anyway?
                let mut has_missing = false;
                let mut elided_targets = vec![];
                for edge in edges {
                    match edge.edge_type {
                        RevsetGraphEdgeType::Missing => {
                            has_missing = true;
                        }
                        RevsetGraphEdgeType::Direct => {
                            graphlog_edges.push(Edge::Direct((edge.target, false)));
                        }
                        RevsetGraphEdgeType::Indirect => {
                            if use_elided_nodes {
                                elided_targets.push(edge.target.clone());
                                graphlog_edges.push(Edge::Direct((edge.target, true)));
                            } else {
                                graphlog_edges.push(Edge::Indirect((edge.target, false)));
                            }
                        }
                    }
                }
                if has_missing {
                    graphlog_edges.push(Edge::Missing);
                }
                let mut buffer = vec![];
                let key = (commit_id, false);
                let commit = store.get_commit(&key.0)?;
                with_content_format.write_graph_text(
                    ui.new_formatter(&mut buffer).as_mut(),
                    |formatter| template.format(&commit, formatter),
                    || graph.width(&key, &graphlog_edges),
                )?;
                if !buffer.ends_with(b"\n") {
                    buffer.push(b'\n');
                }
                if !diff_formats.is_empty() {
                    let mut formatter = ui.new_formatter(&mut buffer);
                    diff_util::show_patch(
                        ui,
                        formatter.as_mut(),
                        &workspace_command,
                        &commit,
                        matcher.as_ref(),
                        &diff_formats,
                    )?;
                }

                let node_symbol = format_template(ui, &Some(commit), &node_template);
                graph.add_node(
                    &key,
                    &graphlog_edges,
                    &node_symbol,
                    &String::from_utf8_lossy(&buffer),
                )?;
                for elided_target in elided_targets {
                    let elided_key = (elided_target, true);
                    let real_key = (elided_key.0.clone(), false);
                    let edges = [Edge::Direct(real_key)];
                    let mut buffer = vec![];
                    with_content_format.write_graph_text(
                        ui.new_formatter(&mut buffer).as_mut(),
                        |formatter| writeln!(formatter.labeled("elided"), "(elided revisions)"),
                        || graph.width(&elided_key, &edges),
                    )?;
                    let node_symbol = format_template(ui, &None, &node_template);
                    graph.add_node(
                        &elided_key,
                        &edges,
                        &node_symbol,
                        &String::from_utf8_lossy(&buffer),
                    )?;
                }
            }
        } else {
            let iter: Box<dyn Iterator<Item = CommitId>> = if args.reversed {
                Box::new(revset.iter().reversed())
            } else {
                Box::new(revset.iter())
            };
            for commit_or_error in iter.commits(store).take(args.limit.unwrap_or(usize::MAX)) {
                let commit = commit_or_error?;
                with_content_format
                    .write(formatter, |formatter| template.format(&commit, formatter))?;
                if !diff_formats.is_empty() {
                    diff_util::show_patch(
                        ui,
                        formatter,
                        &workspace_command,
                        &commit,
                        matcher.as_ref(),
                        &diff_formats,
                    )?;
                }
            }
        }
    }

    // Check to see if the user might have specified a path when they intended
    // to specify a revset.
    if let ([], [only_path]) = (args.revisions.as_slice(), args.paths.as_slice()) {
        if only_path == "." && workspace_command.parse_file_path(only_path)?.is_root() {
            // For users of e.g. Mercurial, where `.` indicates the current commit.
            writeln!(
                ui.warning_default(),
                "The argument {only_path:?} is being interpreted as a path, but this is often not \
                 useful because all non-empty commits touch '.'.  If you meant to show the \
                 working copy commit, pass -r '@' instead."
            )?;
        } else if revset.is_empty()
            && revset::parse(only_path, &workspace_command.revset_parse_context()).is_ok()
        {
            writeln!(
                ui.warning_default(),
                "The argument {only_path:?} is being interpreted as a path. To specify a revset, \
                 pass -r {only_path:?} instead."
            )?;
        }
    }

    Ok(())
}
