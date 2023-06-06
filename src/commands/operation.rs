use clap::Subcommand;
use jujutsu_lib::operation;

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

pub fn cmd_op_undo(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationUndoArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let bad_op = workspace_command.resolve_single_op(&args.operation)?;
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
    let mut tx = workspace_command
        .start_transaction(&format!("restore to operation {}", target_op.id().hex()));
    tx.mut_repo().set_view(target_op.view().take_store_view());
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
