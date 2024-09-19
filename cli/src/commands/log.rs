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

use jj_lib::backend::CommitId;
use jj_lib::graph::GraphEdgeType;
use jj_lib::graph::ReverseGraphIterator;
use jj_lib::graph::TopoGroupedGraphIterator;
use jj_lib::repo::Repo;
use jj_lib::revset::RevsetExpression;
use jj_lib::revset::RevsetFilterPredicate;
use jj_lib::revset::RevsetIteratorExt;
use jj_lib::settings::ConfigResultExt as _;
use jj_lib::settings::UserSettings;
use tracing::instrument;

use crate::cli_util::format_template;
use crate::cli_util::CommandHelper;
use crate::cli_util::LogContentFormat;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::commit_templater::CommitTemplateLanguage;
use crate::diff_util::DiffFormatArgs;
use crate::graphlog::get_graphlog;
use crate::graphlog::Edge;
use crate::graphlog::GraphStyle;
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
    /// Which revisions to show. If no paths nor revisions are specified, this
    /// defaults to the `revsets.log` setting, or `@ |
    /// ancestors(immutable_heads().., 2) | trunk()` if it is not set.
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
    /// For the syntax, see https://martinvonz.github.io/jj/latest/templates/
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

    let fileset_expression = workspace_command.parse_file_patterns(ui, &args.paths)?;
    let revset_expression = {
        // only use default revset if neither revset nor path are specified
        let mut expression = if args.revisions.is_empty() && args.paths.is_empty() {
            workspace_command
                .parse_revset(ui, &RevisionArg::from(command.settings().default_revset()))?
        } else if !args.revisions.is_empty() {
            workspace_command.parse_union_revsets(ui, &args.revisions)?
        } else {
            // a path was specified so we use all() and add path filter later
            workspace_command.attach_revset_evaluator(RevsetExpression::all())
        };
        if !args.paths.is_empty() {
            // Beware that args.paths = ["root:."] is not identical to []. The
            // former will filter out empty commits.
            let predicate = RevsetFilterPredicate::File(fileset_expression.clone());
            expression.intersect_with(&RevsetExpression::filter(predicate));
        }
        expression
    };

    let repo = workspace_command.repo();
    let matcher = fileset_expression.to_matcher();
    let revset = revset_expression.evaluate()?;

    let store = repo.store();
    let diff_renderer = workspace_command.diff_renderer_for_log(&args.diff_format, args.patch)?;
    let graph_style = GraphStyle::from_settings(command.settings())?;

    let use_elided_nodes = command
        .settings()
        .config()
        .get_bool("ui.log-synthetic-elided-nodes")?;
    let with_content_format = LogContentFormat::new(ui, command.settings())?;

    let template;
    let node_template;
    {
        let language = workspace_command.commit_template_language();
        let template_string = match &args.template {
            Some(value) => value.to_string(),
            None => command.settings().config().get_string("templates.log")?,
        };
        template = workspace_command
            .parse_template(
                ui,
                &language,
                &template_string,
                CommitTemplateLanguage::wrap_commit,
            )?
            .labeled("log");
        node_template = workspace_command
            .parse_template(
                ui,
                &language,
                &get_node_template(graph_style, command.settings())?,
                CommitTemplateLanguage::wrap_commit_opt,
            )?
            .labeled("node");
    }

    {
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

        if !args.no_graph {
            let mut graph = get_graphlog(graph_style, formatter.raw());
            let forward_iter = TopoGroupedGraphIterator::new(revset.iter_graph());
            let iter: Box<dyn Iterator<Item = _>> = if args.reversed {
                Box::new(ReverseGraphIterator::new(forward_iter))
            } else {
                Box::new(forward_iter)
            };
            for (commit_id, edges) in iter.take(limit) {
                // The graph is keyed by (CommitId, is_synthetic)
                let mut graphlog_edges = vec![];
                // TODO: Should we update revset.iter_graph() to yield this flag instead of all
                // the missing edges since we don't care about where they point here
                // anyway?
                let mut has_missing = false;
                let mut elided_targets = vec![];
                for edge in edges {
                    match edge.edge_type {
                        GraphEdgeType::Missing => {
                            has_missing = true;
                        }
                        GraphEdgeType::Direct => {
                            graphlog_edges.push(Edge::Direct((edge.target, false)));
                        }
                        GraphEdgeType::Indirect => {
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
                let within_graph =
                    with_content_format.sub_width(graph.width(&key, &graphlog_edges));
                within_graph.write(ui.new_formatter(&mut buffer).as_mut(), |formatter| {
                    template.format(&commit, formatter)
                })?;
                if !buffer.ends_with(b"\n") {
                    buffer.push(b'\n');
                }
                if let Some(renderer) = &diff_renderer {
                    let mut formatter = ui.new_formatter(&mut buffer);
                    renderer.show_patch(
                        ui,
                        formatter.as_mut(),
                        &commit,
                        matcher.as_ref(),
                        within_graph.width(),
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
                    let within_graph =
                        with_content_format.sub_width(graph.width(&elided_key, &edges));
                    within_graph.write(ui.new_formatter(&mut buffer).as_mut(), |formatter| {
                        writeln!(formatter.labeled("elided"), "(elided revisions)")
                    })?;
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
            for commit_or_error in iter.commits(store).take(limit) {
                let commit = commit_or_error?;
                with_content_format
                    .write(formatter, |formatter| template.format(&commit, formatter))?;
                if let Some(renderer) = &diff_renderer {
                    let width = ui.term_width();
                    renderer.show_patch(ui, formatter, &commit, matcher.as_ref(), width)?;
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
            && workspace_command
                .parse_revset(ui, &RevisionArg::from(only_path.to_owned()))
                .is_ok()
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

pub fn get_node_template(
    style: GraphStyle,
    settings: &UserSettings,
) -> Result<String, config::ConfigError> {
    let symbol = settings
        .config()
        .get_string("templates.log_node")
        .optional()?;
    let default = if style.is_ascii() {
        "builtin_log_node_ascii"
    } else {
        "builtin_log_node"
    };
    Ok(symbol.unwrap_or_else(|| default.to_owned()))
}
