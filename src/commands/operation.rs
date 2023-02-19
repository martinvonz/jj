use std::io;

use clap::Subcommand;
use jujutsu_lib::dag_walk::topo_order_reverse;
use jujutsu_lib::operation::Operation;

use crate::cli_util::{user_error, CommandError, CommandHelper};
use crate::formatter::Formatter;
use crate::graphlog::{get_graphlog, Edge};
use crate::templater::{Template, TimestampRange};
use crate::time_util::format_timestamp_relative_to_now;
use crate::ui::Ui;

/// Commands for working with the operation log
///
/// Commands for working with the operation log. For information about the
/// operation log, see https://github.com/martinvonz/jj/blob/main/docs/operation-log.md.
#[derive(Subcommand, Clone, Debug)]
pub enum OperationCommands {
    Log(OperationLogArgs),
    Undo(OperationUndoArgs),
    Restore(OperationRestoreArgs),
}

/// Show the operation log
#[derive(clap::Args, Clone, Debug)]
pub struct OperationLogArgs {}

/// Restore to the state at an operation
#[derive(clap::Args, Clone, Debug)]
pub struct OperationRestoreArgs {
    /// The operation to restore to
    operation: String,
}

/// Undo an operation
#[derive(clap::Args, Clone, Debug)]
pub struct OperationUndoArgs {
    /// The operation to undo
    #[arg(default_value = "@")]
    operation: String,
}

fn cmd_op_log(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &OperationLogArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let head_op = repo.operation().clone();
    let head_op_id = head_op.id().clone();
    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();
    struct OpTemplate {
        relative_timestamps: bool,
    }
    impl Template<Operation> for OpTemplate {
        fn format(&self, op: &Operation, formatter: &mut dyn Formatter) -> io::Result<()> {
            // TODO: Make this templated
            write!(formatter.labeled("id"), "{}", &op.id().hex()[0..12])?;
            formatter.write_str(" ")?;
            let metadata = &op.store_operation().metadata;
            write!(
                formatter.labeled("user"),
                "{}@{}",
                metadata.username,
                metadata.hostname
            )?;
            formatter.write_str(" ")?;
            let time_range = TimestampRange {
                start: metadata.start_time.clone(),
                end: metadata.end_time.clone(),
            };
            if self.relative_timestamps {
                let start = format_timestamp_relative_to_now(&time_range.start);
                write!(
                    formatter.labeled("time"),
                    "{start}, lasted {duration}",
                    duration = time_range.duration()
                )?;
            } else {
                time_range.format(&(), formatter)?;
            }
            formatter.write_str("\n")?;
            write!(
                formatter.labeled("description"),
                "{}",
                &metadata.description
            )?;
            for (key, value) in &metadata.tags {
                write!(formatter.labeled("tags"), "\n{key}: {value}")?;
            }
            Ok(())
        }

        fn has_content(&self, _: &Operation) -> bool {
            true
        }
    }
    let template = OpTemplate {
        relative_timestamps: command.settings().oplog_relative_timestamps(),
    };

    let mut graph = get_graphlog(command.settings(), formatter.raw());
    for op in topo_order_reverse(
        vec![head_op],
        Box::new(|op: &Operation| op.id().clone()),
        Box::new(|op: &Operation| op.parents()),
    ) {
        let mut edges = vec![];
        for parent in op.parents() {
            edges.push(Edge::direct(parent.id().clone()));
        }
        let is_head_op = op.id() == &head_op_id;
        let mut buffer = vec![];
        {
            let mut formatter = ui.new_formatter(&mut buffer);
            formatter.with_label("op-log", |formatter| {
                if is_head_op {
                    formatter.with_label("head", |formatter| template.format(&op, formatter))
                } else {
                    template.format(&op, formatter)
                }
            })?;
        }
        if !buffer.ends_with(b"\n") {
            buffer.push(b'\n');
        }
        let node_symbol = if is_head_op { "@" } else { "o" };
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
