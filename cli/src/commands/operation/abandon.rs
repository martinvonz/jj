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
use std::iter;
use std::slice;

use clap_complete::ArgValueCandidates;
use itertools::Itertools as _;
use jj_lib::op_walk;

use crate::cli_util::short_operation_hash;
use crate::cli_util::CommandHelper;
use crate::command_error::cli_error;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::complete;
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
    #[arg(add = ArgValueCandidates::new(complete::operations))]
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
    let op_heads_store = repo_loader.op_heads_store();
    // It doesn't make sense to create divergent operations that will be merged
    // with the current head.
    if command.global_args().at_operation.is_some() {
        return Err(cli_error("--at-op is not respected"));
    }
    let current_head_ops = op_walk::get_current_head_ops(op_store, op_heads_store.as_ref())?;
    let resolve_op = |op_str| op_walk::resolve_op_at(op_store, &current_head_ops, op_str);
    let (abandon_root_op, abandon_head_ops) =
        if let Some((root_op_str, head_op_str)) = args.operation.split_once("..") {
            let root_op = if root_op_str.is_empty() {
                repo_loader.root_operation()
            } else {
                resolve_op(root_op_str)?
            };
            let head_ops = if head_op_str.is_empty() {
                current_head_ops.clone()
            } else {
                vec![resolve_op(head_op_str)?]
            };
            (root_op, head_ops)
        } else {
            let op = resolve_op(&args.operation)?;
            let parent_ops: Vec<_> = op.parents().try_collect()?;
            let parent_op = match parent_ops.len() {
                0 => return Err(user_error("Cannot abandon the root operation")),
                1 => parent_ops.into_iter().next().unwrap(),
                _ => return Err(user_error("Cannot abandon a merge operation")),
            };
            (parent_op, vec![op])
        };

    if let Some(op) = abandon_head_ops
        .iter()
        .find(|op| current_head_ops.contains(op))
    {
        let mut err = user_error(format!(
            "Cannot abandon the current operation {}",
            short_operation_hash(op.id())
        ));
        if current_head_ops.len() == 1 {
            err.add_hint("Run `jj undo` to revert the current operation, then use `jj op abandon`");
        }
        return Err(err);
    }

    // Reparent descendants, count the number of abandoned operations.
    let stats = op_walk::reparent_range(
        op_store.as_ref(),
        &abandon_head_ops,
        &current_head_ops,
        &abandon_root_op,
    )?;
    assert_eq!(
        current_head_ops.len(),
        stats.new_head_ids.len(),
        "all current_head_ops should be reparented as they aren't included in abandon_head_ops"
    );
    let reparented_head_ops = || iter::zip(&current_head_ops, &stats.new_head_ids);
    if reparented_head_ops().all(|(old, new_id)| old.id() == new_id) {
        writeln!(ui.status(), "Nothing changed.")?;
        return Ok(());
    }
    writeln!(
        ui.status(),
        "Abandoned {} operations and reparented {} descendant operations.",
        stats.unreachable_count,
        stats.rewritten_count,
    )?;
    for (old, new_id) in reparented_head_ops().filter(|&(old, new_id)| old.id() != new_id) {
        op_heads_store.update_op_heads(slice::from_ref(old.id()), new_id)?;
    }
    // Remap the operation id of the current workspace. If there were any
    // divergent operations, user will need to re-abandon their ancestors.
    if !command.global_args().ignore_working_copy {
        let mut locked_ws = workspace.start_working_copy_mutation()?;
        let old_op_id = locked_ws.locked_wc().old_operation_id();
        if let Some((_, new_id)) = reparented_head_ops().find(|(old, _)| old.id() == old_op_id) {
            locked_ws.finish(new_id.clone())?;
        } else {
            writeln!(
                ui.warning_default(),
                "The working copy operation {} is not updated because it differs from the repo {}.",
                short_operation_hash(old_op_id),
                current_head_ops
                    .iter()
                    .map(|op| short_operation_hash(op.id()))
                    .join(", "),
            )?;
        }
    }
    Ok(())
}
