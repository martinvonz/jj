use std::collections::{BTreeMap, BTreeSet};

use clap::Subcommand;
use jj_lib::op_store::{BranchTarget, RefTargetExt as _};
use jj_lib::operation;
use jj_lib::repo::Repo;

use crate::cli_util::{user_error, CommandError, CommandHelper, LogContentFormat};
use crate::graphlog::{get_graphlog, Edge};
use crate::operation_templater;
use crate::templater::Template as _;
use crate::ui::Ui;

/// Commands for working with the operation log
///
/// For information about the operation log, see
/// https://github.com/martinvonz/jj/blob/main/docs/operation-log.md.
#[derive(Subcommand, Clone, Debug)]
pub enum OperationCommands {
    Log(OperationLogArgs),
    Undo(OperationUndoArgs),
    Restore(OperationRestoreArgs),
}

/// Show the operation log
#[derive(clap::Args, Clone, Debug)]
pub struct OperationLogArgs {
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
    /// Defaults to everything for non-colocated repos.
    ///
    /// Defaults to `repo` and `remote-tracking` for colocated repos. This
    /// ensures that the automatic `jj git export` succeeds.
    ///
    /// This option is EXPERIMENTAL.
    #[arg(long)]
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
    /// Defaults to everything for non-colocated repos.
    ///
    /// Defaults to `repo` and `remote-tracking` for colocated repos. This
    /// ensures that the automatic `jj git export` succeeds.
    ///
    /// This option is EXPERIMENTAL.
    #[arg(long)]
    what: Vec<UndoWhatToRestore>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
enum UndoWhatToRestore {
    /// The jj repo state and local branches
    Repo,
    /// The remote-tracking branches. Do not restore these if you'd like to push
    /// after the undo
    RemoteTracking,
    /// Remembered git repo state from the last `jj git import`
    GitTracking,
}

fn cmd_op_log(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationLogArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let head_op = repo.operation().clone();
    let head_op_id = head_op.id().clone();

    let template_string = match &args.template {
        Some(value) => value.to_owned(),
        None => command.settings().config().get_string("templates.op_log")?,
    };
    let template = operation_templater::parse(
        repo,
        &template_string,
        workspace_command.template_aliases_map(),
    )?;
    let with_content_format = LogContentFormat::new(ui, command.settings())?;

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();
    let mut graph = get_graphlog(command.settings(), formatter.raw());
    let default_node_symbol = graph.default_node_symbol().to_owned();
    for op in operation::walk_ancestors(&head_op) {
        let mut edges = vec![];
        for parent in op.parents() {
            edges.push(Edge::direct(parent.id().clone()));
        }
        let is_head_op = op.id() == &head_op_id;
        let mut buffer = vec![];
        with_content_format.write_graph_text(
            ui.new_formatter(&mut buffer).as_mut(),
            |formatter| formatter.with_label("op_log", |formatter| template.format(&op, formatter)),
            || graph.width(op.id(), &edges),
        )?;
        if !buffer.ends_with(b"\n") {
            buffer.push(b'\n');
        }
        let node_symbol = if is_head_op {
            "@"
        } else {
            &default_node_symbol
        };
        graph.add_node(
            op.id(),
            &edges,
            node_symbol,
            &String::from_utf8_lossy(&buffer),
        )?;
    }

    Ok(())
}

/// Restore only the portions of the view specified by the `what` argument
fn view_with_desired_portions_restored(
    view_being_restored: &jj_lib::op_store::View,
    current_view: &jj_lib::op_store::View,
    what: &[UndoWhatToRestore],
) -> jj_lib::op_store::View {
    let mut new_view = if what.contains(&UndoWhatToRestore::Repo) {
        view_being_restored.clone()
    } else {
        current_view.clone()
    };
    new_view.git_refs = if what.contains(&UndoWhatToRestore::GitTracking) {
        view_being_restored.git_refs.clone()
    } else {
        current_view.git_refs.clone()
    };

    if what.contains(&UndoWhatToRestore::RemoteTracking) == what.contains(&UndoWhatToRestore::Repo)
    {
        // new_view already contains the correct branches; we can short-curcuit
        return new_view;
    }

    let all_branch_names: BTreeSet<_> = itertools::chain(
        view_being_restored.branches.keys(),
        current_view.branches.keys(),
    )
    .collect();
    let branch_source_view = if what.contains(&UndoWhatToRestore::RemoteTracking) {
        view_being_restored
    } else {
        current_view
    };
    let mut new_branches = BTreeMap::default();
    for branch_name in all_branch_names {
        let local_target = new_view
            .branches
            .get(branch_name)
            .and_then(|br| br.local_target.clone());
        let remote_targets = branch_source_view
            .branches
            .get(branch_name)
            .map(|br| br.remote_targets.clone())
            .unwrap_or_default();
        if local_target.is_present() || !remote_targets.is_empty() {
            new_branches.insert(
                branch_name.to_string(),
                BranchTarget {
                    local_target,
                    remote_targets,
                },
            );
        }
    }
    new_view.branches = new_branches;
    new_view
}

fn process_what_arg(what_arg: &[UndoWhatToRestore], colocated: bool) -> Vec<UndoWhatToRestore> {
    if !what_arg.is_empty() {
        what_arg.to_vec()
    } else {
        let mut default_what = vec![UndoWhatToRestore::Repo, UndoWhatToRestore::RemoteTracking];
        if !colocated {
            // In a colocated repo, restoring the git-tracking refs is harmful
            // (https://github.com/martinvonz/jj/issues/922).
            //
            // The issue is that `jj undo` does not directly change the local
            // git repo's branches. Keeping those up to date the job of the
            // automatic `jj git import` and `jj git export`, and they rely on the
            // git-tracking refs matching the git repo's branches.
            //
            // Consider, for example, undoing a `jj branch set` command. If the
            // git-tracking refs were restored by `undo`, they would no longer
            // match the actual positions of branches in the git repo. So, the
            // automatic `jj git export` would fail and the automatic `jj git
            // import` would create a conflict, as demonstrated by the bug
            // linked above.
            //
            // So, we have `undo` *not* move the git-tracking branches. After
            // the undo, git-tracking refs will still match the actual positions
            // of the git repo's branches (in the normal case where they matched
            // before the undo). The automatic `jj git export` that happens
            // immediately after the undo will successfully export whatever
            // changes to branches `undo` caused.
            default_what.push(UndoWhatToRestore::GitTracking);
        }
        default_what
    }
}

pub fn cmd_op_undo(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationUndoArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let bad_op = workspace_command.resolve_single_op(&args.operation)?;
    let repo_is_colocated = workspace_command.working_copy_shared_with_git();
    let parent_ops = bad_op.parents();
    if parent_ops.len() > 1 {
        return Err(user_error("Cannot undo a merge operation"));
    }
    if parent_ops.is_empty() {
        return Err(user_error("Cannot undo repo initialization"));
    }

    let mut tx =
        workspace_command.start_transaction(&format!("undo operation {}", bad_op.id().hex()));
    let repo_loader = tx.base_repo().loader();
    let bad_repo = repo_loader.load_at(&bad_op);
    let parent_repo = repo_loader.load_at(&parent_ops[0]);
    tx.mut_repo().merge(&bad_repo, &parent_repo);
    let new_view = view_with_desired_portions_restored(
        tx.repo().view().store_view(),
        tx.base_repo().view().store_view(),
        &process_what_arg(&args.what, repo_is_colocated),
    );
    tx.mut_repo().set_view(new_view);
    tx.finish(ui)?;

    Ok(())
}

fn cmd_op_restore(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationRestoreArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let target_op = workspace_command.resolve_single_op(&args.operation)?;
    let repo_is_colocated = workspace_command.working_copy_shared_with_git();
    let mut tx = workspace_command
        .start_transaction(&format!("restore to operation {}", target_op.id().hex()));
    let new_view = view_with_desired_portions_restored(
        target_op.view().store_view(),
        tx.base_repo().view().store_view(),
        &process_what_arg(&args.what, repo_is_colocated),
    );
    tx.mut_repo().set_view(new_view);
    tx.finish(ui)?;

    Ok(())
}

pub fn cmd_operation(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &OperationCommands,
) -> Result<(), CommandError> {
    match subcommand {
        OperationCommands::Log(command_matches) => cmd_op_log(ui, command, command_matches),
        OperationCommands::Restore(command_matches) => cmd_op_restore(ui, command, command_matches),
        OperationCommands::Undo(command_matches) => cmd_op_undo(ui, command, command_matches),
    }
}
