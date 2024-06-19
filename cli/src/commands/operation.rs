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

use std::collections::HashMap;
use std::io::Write as _;
use std::slice;
use std::sync::Arc;

use clap::Subcommand;
use indexmap::IndexMap;
use itertools::Itertools as _;
use jj_lib::backend::{BackendResult, ChangeId, CommitId};
use jj_lib::commit::Commit;
use jj_lib::git::REMOTE_NAME_FOR_LOCAL_GIT_REPO;
use jj_lib::graph::{GraphEdge, TopoGroupedGraphIterator};
use jj_lib::matchers::EverythingMatcher;
use jj_lib::object_id::ObjectId;
use jj_lib::op_store::{OpStoreResult, OperationId, RefTarget, RemoteRef, RemoteRefState};
use jj_lib::operation::Operation;
use jj_lib::refs::{diff_named_ref_targets, diff_named_remote_refs};
use jj_lib::repo::{MutableRepo, ReadonlyRepo, Repo, RepoLoader};
use jj_lib::revset::RevsetIteratorExt;
use jj_lib::rewrite::rebase_to_dest_parent;
use jj_lib::settings::UserSettings;
use jj_lib::{dag_walk, op_walk, revset};

use crate::cli_util::{
    format_template, short_change_hash, short_operation_hash, CommandHelper, LogContentFormat,
    WorkspaceCommandTransaction,
};
use crate::command_error::{user_error, user_error_with_hint, CommandError};
use crate::diff_util::{DiffFormatArgs, DiffRenderer};
use crate::formatter::Formatter;
use crate::graphlog::{get_graphlog, Edge};
use crate::operation_templater::OperationTemplateLanguage;
use crate::ui::Ui;

/// Commands for working with the operation log
///
/// For information about the operation log, see
/// https://github.com/martinvonz/jj/blob/main/docs/operation-log.md.
#[derive(Subcommand, Clone, Debug)]
pub enum OperationCommand {
    Abandon(OperationAbandonArgs),
    Diff(OperationDiffArgs),
    Log(OperationLogArgs),
    Show(OperationShowArgs),
    Undo(OperationUndoArgs),
    Restore(OperationRestoreArgs),
}

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

/// Create a new operation that restores the repo to an earlier state
///
/// This restores the repo to the state at the specified operation, effectively
/// undoing all later operations. It does so by creating a new operation.
#[derive(clap::Args, Clone, Debug)]
pub struct OperationRestoreArgs {
    /// The operation to restore to
    ///
    /// Use `jj op log` to find an operation to restore to. Use e.g. `jj
    /// --at-op=<operation ID> log` before restoring to an operation to see the
    /// state of the repo at that operation.
    operation: String,

    /// What portions of the local state to restore (can be repeated)
    ///
    /// This option is EXPERIMENTAL.
    #[arg(long, value_enum, default_values_t = DEFAULT_UNDO_WHAT)]
    what: Vec<UndoWhatToRestore>,
}

/// Create a new operation that undoes an earlier operation
///
/// This undoes an individual operation by applying the inverse of the
/// operation.
#[derive(clap::Args, Clone, Debug)]
pub struct OperationUndoArgs {
    /// The operation to undo
    ///
    /// Use `jj op log` to find an operation to undo.
    #[arg(default_value = "@")]
    operation: String,

    /// What portions of the local state to restore (can be repeated)
    ///
    /// This option is EXPERIMENTAL.
    #[arg(long, value_enum, default_values_t = DEFAULT_UNDO_WHAT)]
    what: Vec<UndoWhatToRestore>,
}

/// Abandon operation history
///
/// To discard old operation history, use `jj op abandon ..<operation ID>`. It
/// will abandon the specified operation and all its ancestors. The descendants
/// will be reparented onto the root operation.
///
/// To discard recent operations, use `jj op restore <operation ID>` followed
/// by `jj op abandon <operation ID>..@-`.
///
/// The abandoned operations, commits, and other unreachable objects can later
/// be garbage collected by using `jj util gc` command.
#[derive(clap::Args, Clone, Debug)]
pub struct OperationAbandonArgs {
    /// The operation or operation range to abandon
    operation: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
enum UndoWhatToRestore {
    /// The jj repo state and local branches
    Repo,
    /// The remote-tracking branches. Do not restore these if you'd like to push
    /// after the undo
    RemoteTracking,
}

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

/// Compare changes to the repository between two operations
#[derive(clap::Args, Clone, Debug)]
pub struct OperationDiffArgs {
    /// Show repository changes in this operation, compared to its parent
    #[arg(long)]
    operation: Option<String>,
    /// Show repository changes from this operation
    #[arg(long, conflicts_with = "operation")]
    from: Option<String>,
    /// Show repository changes to this operation
    #[arg(long, conflicts_with = "operation")]
    to: Option<String>,
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

const DEFAULT_UNDO_WHAT: [UndoWhatToRestore; 2] =
    [UndoWhatToRestore::Repo, UndoWhatToRestore::RemoteTracking];

fn cmd_op_log(
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

/// Restore only the portions of the view specified by the `what` argument
fn view_with_desired_portions_restored(
    view_being_restored: &jj_lib::op_store::View,
    current_view: &jj_lib::op_store::View,
    what: &[UndoWhatToRestore],
) -> jj_lib::op_store::View {
    let repo_source = if what.contains(&UndoWhatToRestore::Repo) {
        view_being_restored
    } else {
        current_view
    };
    let remote_source = if what.contains(&UndoWhatToRestore::RemoteTracking) {
        view_being_restored
    } else {
        current_view
    };
    jj_lib::op_store::View {
        head_ids: repo_source.head_ids.clone(),
        local_branches: repo_source.local_branches.clone(),
        tags: repo_source.tags.clone(),
        remote_views: remote_source.remote_views.clone(),
        git_refs: current_view.git_refs.clone(),
        git_head: current_view.git_head.clone(),
        wc_commit_ids: repo_source.wc_commit_ids.clone(),
    }
}

pub fn cmd_op_undo(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationUndoArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let bad_op = workspace_command.resolve_single_op(&args.operation)?;
    let mut parent_ops = bad_op.parents();
    let Some(parent_op) = parent_ops.next().transpose()? else {
        return Err(user_error("Cannot undo repo initialization"));
    };
    if parent_ops.next().is_some() {
        return Err(user_error("Cannot undo a merge operation"));
    }

    let mut tx = workspace_command.start_transaction();
    let repo_loader = tx.base_repo().loader();
    let bad_repo = repo_loader.load_at(&bad_op)?;
    let parent_repo = repo_loader.load_at(&parent_op)?;
    tx.mut_repo().merge(&bad_repo, &parent_repo);
    let new_view = view_with_desired_portions_restored(
        tx.repo().view().store_view(),
        tx.base_repo().view().store_view(),
        &args.what,
    );
    tx.mut_repo().set_view(new_view);
    tx.finish(ui, format!("undo operation {}", bad_op.id().hex()))?;

    Ok(())
}

fn cmd_op_restore(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationRestoreArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let target_op = workspace_command.resolve_single_op(&args.operation)?;
    let mut tx = workspace_command.start_transaction();
    let new_view = view_with_desired_portions_restored(
        target_op.view()?.store_view(),
        tx.base_repo().view().store_view(),
        &args.what,
    );
    tx.mut_repo().set_view(new_view);
    tx.finish(ui, format!("restore to operation {}", target_op.id().hex()))?;

    Ok(())
}

fn cmd_op_abandon(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationAbandonArgs,
) -> Result<(), CommandError> {
    // Don't load the repo so that this command can be used to recover from
    // corrupted repo state.
    let mut workspace = command.load_workspace()?;
    let repo_loader = workspace.repo_loader();
    let op_store = repo_loader.op_store();
    // It doesn't make sense to create concurrent operations that will be merged
    // with the current head.
    let head_op_str = &command.global_args().at_operation;
    if head_op_str != "@" {
        return Err(user_error("--at-op is not respected"));
    }
    let current_head_op = op_walk::resolve_op_for_load(repo_loader, head_op_str)?;
    let resolve_op = |op_str| op_walk::resolve_op_at(op_store, &current_head_op, op_str);
    let (abandon_root_op, abandon_head_op) =
        if let Some((root_op_str, head_op_str)) = args.operation.split_once("..") {
            let root_op = if root_op_str.is_empty() {
                let id = op_store.root_operation_id();
                let data = op_store.read_operation(id)?;
                Operation::new(op_store.clone(), id.clone(), data)
            } else {
                resolve_op(root_op_str)?
            };
            let head_op = if head_op_str.is_empty() {
                current_head_op.clone()
            } else {
                resolve_op(head_op_str)?
            };
            (root_op, head_op)
        } else {
            let op = resolve_op(&args.operation)?;
            let parent_ops: Vec<_> = op.parents().try_collect()?;
            let parent_op = match parent_ops.len() {
                0 => return Err(user_error("Cannot abandon the root operation")),
                1 => parent_ops.into_iter().next().unwrap(),
                _ => return Err(user_error("Cannot abandon a merge operation")),
            };
            (parent_op, op)
        };

    if abandon_head_op == current_head_op {
        return Err(user_error_with_hint(
            "Cannot abandon the current operation",
            "Run `jj undo` to revert the current operation, then use `jj op abandon`",
        ));
    }

    // Reparent descendants, count the number of abandoned operations.
    let stats = op_walk::reparent_range(
        op_store.as_ref(),
        slice::from_ref(&abandon_head_op),
        slice::from_ref(&current_head_op),
        &abandon_root_op,
    )?;
    let [new_head_id]: [OperationId; 1] = stats.new_head_ids.try_into().unwrap();
    if current_head_op.id() == &new_head_id {
        writeln!(ui.status(), "Nothing changed.")?;
        return Ok(());
    }
    writeln!(
        ui.status(),
        "Abandoned {} operations and reparented {} descendant operations.",
        stats.unreachable_count,
        stats.rewritten_count,
    )?;
    repo_loader
        .op_heads_store()
        .update_op_heads(slice::from_ref(current_head_op.id()), &new_head_id);
    // Remap the operation id of the current workspace. If there were any
    // concurrent operations, user will need to re-abandon their ancestors.
    if !command.global_args().ignore_working_copy {
        let mut locked_ws = workspace.start_working_copy_mutation()?;
        let old_op_id = locked_ws.locked_wc().old_operation_id();
        if old_op_id != current_head_op.id() {
            writeln!(
                ui.warning_default(),
                "The working copy operation {} is not updated because it differs from the repo {}.",
                short_operation_hash(old_op_id),
                short_operation_hash(current_head_op.id()),
            )?;
        } else {
            locked_ws.finish(new_head_id)?
        }
    }
    Ok(())
}

fn cmd_op_show(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationShowArgs,
) -> Result<(), CommandError> {
    // TODO: Should we load the repo here?
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
    let op = op_walk::resolve_op_for_load(repo_loader, &args.operation)?;
    let parent_op = merge_operations(repo_loader, command.settings(), op.parents())?;
    if parent_op.is_none() {
        return Err(user_error("Cannot show the root operation"));
    }
    let parent_op = parent_op.unwrap();
    let with_content_format = LogContentFormat::new(ui, command.settings())?;

    // TODO: Should we make this customizable via clap arg?
    let template;
    {
        let language = OperationTemplateLanguage::new(
            repo_loader.op_store().root_operation_id(),
            current_op_id,
            command.operation_template_extensions(),
        );
        let text = command.settings().config().get_string("templates.op_log")?;
        template = command
            .parse_template(
                ui,
                &language,
                &text,
                OperationTemplateLanguage::wrap_operation,
            )?
            .labeled("op_log");
    }

    let parent_repo = repo_loader.load_at(&parent_op)?;
    let repo = repo_loader.load_at(&op)?;

    ui.request_pager();
    template.format(&op, ui.stdout_formatter().as_mut())?;
    writeln!(ui.stdout())?;

    show_op_diff(
        ui,
        command,
        &parent_repo,
        &repo,
        !args.no_graph,
        &with_content_format,
        &args.diff_format,
        args.patch,
    )
}

fn cmd_op_diff(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationDiffArgs,
) -> Result<(), CommandError> {
    // TODO: Should we load the repo here?
    let workspace = command.load_workspace()?;
    let repo_loader = workspace.repo_loader();
    let head_op_str = &command.global_args().at_operation;
    let from_op;
    let to_op;
    if args.from.is_some() || args.to.is_some() {
        from_op =
            op_walk::resolve_op_for_load(repo_loader, args.from.as_ref().unwrap_or(head_op_str))?;
        to_op = op_walk::resolve_op_for_load(repo_loader, args.to.as_ref().unwrap_or(head_op_str))?;
    } else {
        to_op = op_walk::resolve_op_for_load(
            repo_loader,
            args.operation.as_ref().unwrap_or(head_op_str),
        )?;
        let merged_parents_op = merge_operations(repo_loader, command.settings(), to_op.parents())?;
        if merged_parents_op.is_none() {
            return Err(user_error("Cannot diff operation with no parents"));
        }
        from_op = merged_parents_op.unwrap();
    }
    let with_content_format = LogContentFormat::new(ui, command.settings())?;

    let from_repo = repo_loader.load_at(&from_op)?;
    let to_repo = repo_loader.load_at(&to_op)?;

    ui.request_pager();
    writeln!(
        ui.stdout(),
        "From operation {}: {}",
        short_operation_hash(from_op.id()),
        from_op.metadata().description,
    )?;
    writeln!(
        ui.stdout(),
        "  To operation {}: {}",
        short_operation_hash(to_op.id()),
        to_op.metadata().description,
    )?;
    writeln!(ui.stdout())?;

    show_op_diff(
        ui,
        command,
        &from_repo,
        &to_repo,
        !args.no_graph,
        &with_content_format,
        &args.diff_format,
        args.patch,
    )
}

// Merges the given `operations` into a single operation. Returns `None` if
// there are no operations to merge.
fn merge_operations(
    repo_loader: &RepoLoader,
    settings: &UserSettings,
    mut operations: impl ExactSizeIterator<Item = OpStoreResult<Operation>>,
) -> Result<Option<Operation>, CommandError> {
    let num_operations = operations.len();
    if num_operations == 0 {
        return Ok(None);
    }

    let base_op = operations.next().transpose()?.unwrap();
    let final_op = if num_operations > 1 {
        let base_repo = repo_loader.load_at(&base_op)?;
        let mut tx = base_repo.start_transaction(settings);
        for other_op in operations {
            let other_op = other_op?;
            tx.merge_operation(other_op)?;
            tx.mut_repo().rebase_descendants(settings)?;
        }
        let tx_description = format!("merge {} operations", num_operations);
        let merged_repo = tx.write(tx_description).leave_unpublished();
        merged_repo.operation().clone()
    } else {
        base_op
    };

    Ok(Some(final_op))
}

// Computes and shows the differences between two operations, using the given
// `Repo`s for the operations.
#[allow(clippy::too_many_arguments)]
fn show_op_diff(
    ui: &mut Ui,
    command: &CommandHelper,
    from_repo: &Arc<ReadonlyRepo>,
    to_repo: &Arc<ReadonlyRepo>,
    show_graph: bool,
    with_content_format: &LogContentFormat,
    diff_format_args: &DiffFormatArgs,
    patch: bool,
) -> Result<(), CommandError> {
    let diff_workspace_command =
        command.for_loaded_repo(ui, command.load_workspace()?, to_repo.clone())?;
    let diff_renderer = diff_workspace_command.diff_renderer_for_log(diff_format_args, patch)?;

    // Create a new transaction starting from `to_repo`.
    let mut workspace_command =
        command.for_loaded_repo(ui, command.load_workspace()?, to_repo.clone())?;
    let mut tx = workspace_command.start_transaction();
    // Merge index from `from_repo` to `to_repo`, so commits in `from_repo` are
    // accessible.
    tx.mut_repo().merge_index(from_repo);

    let changes = compute_operation_commits_diff(tx.mut_repo(), from_repo, to_repo)?;

    let commit_id_change_id_map: HashMap<CommitId, ChangeId> = changes
        .iter()
        .flat_map(|(change_id, modified_change)| {
            modified_change
                .added_commits
                .iter()
                .map(|commit| (commit.id().clone(), change_id.clone()))
                .chain(
                    modified_change
                        .removed_commits
                        .iter()
                        .map(|commit| (commit.id().clone(), change_id.clone())),
                )
        })
        .collect();

    let change_parents: HashMap<_, _> = changes
        .iter()
        .map(|(change_id, modified_change)| {
            let parent_change_ids = get_parent_changes(modified_change, &commit_id_change_id_map);
            (change_id.clone(), parent_change_ids)
        })
        .collect();

    // Order changes in reverse topological order.
    let ordered_changes = dag_walk::topo_order_reverse(
        changes.keys().cloned().collect_vec(),
        |change_id: &ChangeId| change_id.clone(),
        |change_id: &ChangeId| change_parents.get(change_id).unwrap().clone(),
    );

    let graph_iter = TopoGroupedGraphIterator::new(ordered_changes.iter().map(|change_id| {
        let parent_change_ids = change_parents.get(change_id).unwrap();
        (
            change_id.clone(),
            parent_change_ids
                .iter()
                .map(|parent_change_id| GraphEdge::direct(parent_change_id.clone()))
                .collect_vec(),
        )
    }));

    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();

    if !ordered_changes.is_empty() {
        writeln!(formatter, "Changed commits:")?;
        if show_graph {
            let mut graph = get_graphlog(command.settings(), formatter.raw());
            for (change_id, edges) in graph_iter {
                let modified_change = changes.get(&change_id).unwrap();
                let edges = edges
                    .iter()
                    .map(|edge| Edge::Direct(edge.target.clone()))
                    .collect_vec();

                let mut buffer = vec![];
                with_content_format.write_graph_text(
                    ui.new_formatter(&mut buffer).as_mut(),
                    |formatter| {
                        write_modified_change_summary(formatter, &tx, &change_id, modified_change)
                    },
                    || graph.width(&change_id, &edges),
                )?;
                if !buffer.ends_with(b"\n") {
                    buffer.push(b'\n');
                }
                if let Some(diff_renderer) = &diff_renderer {
                    let mut formatter = ui.new_formatter(&mut buffer);
                    show_change_diff(ui, formatter.as_mut(), &tx, diff_renderer, modified_change)?;
                }

                // TODO: customize node symbol?
                let node_symbol = "○";
                graph.add_node(
                    &change_id,
                    &edges,
                    node_symbol,
                    &String::from_utf8_lossy(&buffer),
                )?;
            }
        } else {
            for (change_id, _) in graph_iter {
                let modified_change = changes.get(&change_id).unwrap();
                write_modified_change_summary(formatter, &tx, &change_id, modified_change)?;
                if let Some(diff_renderer) = &diff_renderer {
                    show_change_diff(ui, formatter, &tx, diff_renderer, modified_change)?;
                }
            }
        }
        writeln!(formatter)?;
    }

    let changed_local_branches = diff_named_ref_targets(
        from_repo.view().local_branches(),
        to_repo.view().local_branches(),
    )
    .collect_vec();
    if !changed_local_branches.is_empty() {
        writeln!(formatter, "Changed local branches:")?;
        for (name, (from_target, to_target)) in changed_local_branches {
            writeln!(formatter, "{}:", name)?;
            write_ref_target_summary(formatter, &tx, "+", to_target)?;
            write_ref_target_summary(formatter, &tx, "-", from_target)?;
        }
        writeln!(formatter)?;
    }

    let changed_tags =
        diff_named_ref_targets(from_repo.view().tags(), to_repo.view().tags()).collect_vec();
    if !changed_tags.is_empty() {
        writeln!(formatter, "Changed tags:")?;
        for (name, (from_target, to_target)) in changed_tags {
            writeln!(formatter, "{}:", name)?;
            write_ref_target_summary(formatter, &tx, "+", to_target)?;
            write_ref_target_summary(formatter, &tx, "-", from_target)?;
        }
        writeln!(formatter)?;
    }

    let changed_remote_branches = diff_named_remote_refs(
        from_repo.view().all_remote_branches(),
        to_repo.view().all_remote_branches(),
    )
    // Skip updates to local git repo, since they should typically be covered in
    // local branches.
    .filter(|((_, remote_name), _)| *remote_name != REMOTE_NAME_FOR_LOCAL_GIT_REPO)
    .collect_vec();
    if !changed_remote_branches.is_empty() {
        writeln!(formatter, "Changed remote branches:")?;
        let format_remote_ref_prefix = |prefix: &str, remote_ref: &RemoteRef| {
            format!(
                "{} ({})",
                prefix,
                match remote_ref.state {
                    RemoteRefState::New => "untracked",
                    RemoteRefState::Tracking => "tracked",
                }
            )
        };
        for ((name, remote_name), (from_ref, to_ref)) in changed_remote_branches {
            writeln!(formatter, "{}@{}:", name, remote_name)?;
            write_ref_target_summary(
                formatter,
                &tx,
                &format_remote_ref_prefix("+", to_ref),
                &to_ref.target,
            )?;
            write_ref_target_summary(
                formatter,
                &tx,
                &format_remote_ref_prefix("-", from_ref),
                &from_ref.target,
            )?;
        }
    }

    Ok(())
}

// Writes a summary for the given `ModifiedChange`.
fn write_modified_change_summary(
    formatter: &mut dyn Formatter,
    tx: &WorkspaceCommandTransaction,
    change_id: &ChangeId,
    modified_change: &ModifiedChange,
) -> Result<(), std::io::Error> {
    writeln!(
        formatter,
        "Modified change {}",
        short_change_hash(change_id)
    )?;
    for commit in modified_change.added_commits.iter() {
        write!(formatter, "+")?;
        tx.write_commit_summary(formatter, commit)?;
        writeln!(formatter)?;
    }
    for commit in modified_change.removed_commits.iter() {
        write!(formatter, "-")?;
        tx.write_commit_summary(formatter, commit)?;
        writeln!(formatter)?;
    }
    Ok(())
}

// Writes a summary for the given `RefTarget`.
fn write_ref_target_summary(
    formatter: &mut dyn Formatter,
    tx: &WorkspaceCommandTransaction,
    prefix: &str,
    ref_target: &RefTarget,
) -> Result<(), CommandError> {
    if ref_target.is_absent() {
        writeln!(formatter, "{} (absent)", prefix)?;
    } else if ref_target.has_conflict() {
        for commit_id in ref_target.added_ids() {
            write!(formatter, "{} (added) ", prefix)?;
            let commit = tx.repo().store().get_commit(commit_id)?;
            tx.write_commit_summary(formatter, &commit)?;
            writeln!(formatter)?;
        }
        for commit_id in ref_target.removed_ids() {
            write!(formatter, "{} (removed) ", prefix)?;
            let commit = tx.repo().store().get_commit(commit_id)?;
            tx.write_commit_summary(formatter, &commit)?;
            writeln!(formatter)?;
        }
    } else {
        write!(formatter, "{} ", prefix)?;
        let commit_id = ref_target.as_normal().unwrap();
        let commit = tx.repo().store().get_commit(commit_id)?;
        tx.write_commit_summary(formatter, &commit)?;
        writeln!(formatter)?;
    }
    Ok(())
}

// Returns the change IDs of the parents of the given `modified_change`, which
// are the parents of all newly added commits for the change, or the parents of
// all removed commits if there are no added commits.
fn get_parent_changes(
    modified_change: &ModifiedChange,
    commit_id_change_id_map: &HashMap<CommitId, ChangeId>,
) -> Vec<ChangeId> {
    // TODO: how should we handle multiple added or removed commits?
    // This logic is probably slightly iffy.
    if !modified_change.added_commits.is_empty() {
        modified_change
            .added_commits
            .iter()
            .flat_map(|commit| commit.parent_ids())
            .filter_map(|parent_id| commit_id_change_id_map.get(parent_id).cloned())
            .unique()
            .collect_vec()
    } else {
        modified_change
            .removed_commits
            .iter()
            .flat_map(|commit| commit.parent_ids())
            .filter_map(|parent_id| commit_id_change_id_map.get(parent_id).cloned())
            .unique()
            .collect_vec()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ModifiedChange {
    added_commits: Vec<Commit>,
    removed_commits: Vec<Commit>,
}

// Compute the changes in commits between two operations, returned as a
// `HashMap` from `ChangeId` to a `ModifiedChange` struct containing the added
// and removed commits for the change ID.
fn compute_operation_commits_diff(
    repo: &MutableRepo,
    from_repo: &ReadonlyRepo,
    to_repo: &ReadonlyRepo,
) -> BackendResult<IndexMap<ChangeId, ModifiedChange>> {
    let mut changes: IndexMap<ChangeId, ModifiedChange> = IndexMap::new();

    let from_heads = from_repo.view().heads().iter().cloned().collect_vec();
    let to_heads = to_repo.view().heads().iter().cloned().collect_vec();

    // Find newly added commits in `to_repo` which were not present in
    // `from_repo`.
    for commit in revset::walk_revs(repo, &to_heads, &from_heads)
        .unwrap()
        .iter()
        .commits(repo.store())
    {
        let commit = commit?;
        let modified_change = changes
            .entry(commit.change_id().clone())
            .or_insert_with(|| ModifiedChange {
                added_commits: vec![],
                removed_commits: vec![],
            });
        modified_change.added_commits.push(commit);
    }

    // Find commits which were hidden in `to_repo`.
    for commit in revset::walk_revs(repo, &from_heads, &to_heads)
        .unwrap()
        .iter()
        .commits(repo.store())
    {
        let commit = commit?;
        let modified_change = changes
            .entry(commit.change_id().clone())
            .or_insert_with(|| ModifiedChange {
                added_commits: vec![],
                removed_commits: vec![],
            });
        modified_change.removed_commits.push(commit);
    }

    Ok(changes)
}

// Displays the diffs of a modified change. The output differs based on the
// commits added and removed for the change.
// If there is a single added and removed commit, the diff is shown between the
// removed commit and the added commit rebased onto the removed commit's
// parents. If there is only a single added or single removed commit, the diff
// is shown of that commit's contents.
fn show_change_diff(
    ui: &Ui,
    formatter: &mut dyn Formatter,
    tx: &WorkspaceCommandTransaction,
    diff_renderer: &DiffRenderer,
    modified_change: &ModifiedChange,
) -> Result<(), CommandError> {
    // TODO: how should we handle multiple added or removed commits?
    // Alternatively, use `predecessors`?
    if modified_change.added_commits.len() == 1 && modified_change.removed_commits.len() == 1 {
        let commit = &modified_change.added_commits[0];
        let predecessor = &modified_change.removed_commits[0];
        let predecessor_tree = rebase_to_dest_parent(tx.repo(), predecessor, commit)?;
        let tree = commit.tree()?;
        diff_renderer.show_diff(ui, formatter, &predecessor_tree, &tree, &EverythingMatcher)?;
    }
    // TODO: Should we even show a diff for added or removed commits?
    else if modified_change.added_commits.len() == 1 {
        let commit = &modified_change.added_commits[0];
        diff_renderer.show_patch(ui, formatter, commit, &EverythingMatcher)?;
    } else if modified_change.removed_commits.len() == 1 {
        let commit = &modified_change.removed_commits[0];
        diff_renderer.show_patch(ui, formatter, commit, &EverythingMatcher)?;
    }

    Ok(())
}

pub fn cmd_operation(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &OperationCommand,
) -> Result<(), CommandError> {
    match subcommand {
        OperationCommand::Abandon(args) => cmd_op_abandon(ui, command, args),
        OperationCommand::Diff(args) => cmd_op_diff(ui, command, args),
        OperationCommand::Log(args) => cmd_op_log(ui, command, args),
        OperationCommand::Show(args) => cmd_op_show(ui, command, args),
        OperationCommand::Restore(args) => cmd_op_restore(ui, command, args),
        OperationCommand::Undo(args) => cmd_op_undo(ui, command, args),
    }
}
