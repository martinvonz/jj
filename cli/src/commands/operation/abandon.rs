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

use std::io::Write as _;
use std::slice;

use itertools::Itertools as _;
use jj_lib::op_store::OperationId;
use jj_lib::op_walk;
use jj_lib::operation::Operation;

use crate::cli_util::{short_operation_hash, CommandHelper};
use crate::command_error::{user_error, user_error_with_hint, CommandError};
use crate::ui::Ui;

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

pub fn cmd_op_abandon(
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
