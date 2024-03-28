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

use std::io::Write;

use indexmap::IndexMap;
use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::repo::Repo;
use tracing::instrument;

use crate::cli_util::{short_commit_hash, CommandHelper, RevisionArg};
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

/// Create a new change with the same content as an existing one
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct DuplicateArgs {
    /// The revision(s) to duplicate
    #[arg(default_value = "@")]
    revisions: Vec<RevisionArg>,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true, action = clap::ArgAction::Count)]
    unused_revision: u8,
}

#[instrument(skip_all)]
pub(crate) fn cmd_duplicate(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DuplicateArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let to_duplicate: Vec<CommitId> = {
        let expression = workspace_command.parse_union_revsets(&args.revisions)?;
        let revset = workspace_command.evaluate_revset(expression)?;
        revset.iter().collect() // in reverse topological order
    };
    if to_duplicate.is_empty() {
        writeln!(ui.stderr(), "No revisions to duplicate.")?;
        return Ok(());
    }
    if to_duplicate.last() == Some(workspace_command.repo().store().root_commit_id()) {
        return Err(user_error("Cannot duplicate the root commit"));
    }
    let mut duplicated_old_to_new: IndexMap<&CommitId, Commit> = IndexMap::new();

    let mut tx = workspace_command.start_transaction();
    let base_repo = tx.base_repo().clone();
    let store = base_repo.store();
    let mut_repo = tx.mut_repo();

    for original_commit_id in to_duplicate.iter().rev() {
        // Topological order ensures that any parents of `original_commit` are
        // either not in `to_duplicate` or were already duplicated.
        let original_commit = store.get_commit(original_commit_id)?;
        let new_parents = original_commit
            .parent_ids()
            .iter()
            .map(|id| duplicated_old_to_new.get(id).map_or(id, |c| c.id()).clone())
            .collect();
        let new_commit = mut_repo
            .rewrite_commit(command.settings(), &original_commit)
            .generate_new_change_id()
            .set_parents(new_parents)
            .write()?;
        duplicated_old_to_new.insert(original_commit_id, new_commit);
    }

    for (old_id, new_commit) in &duplicated_old_to_new {
        write!(ui.stderr(), "Duplicated {} as ", short_commit_hash(old_id))?;
        tx.write_commit_summary(ui.stderr_formatter().as_mut(), new_commit)?;
        writeln!(ui.stderr())?;
    }
    tx.finish(ui, format!("duplicate {} commit(s)", to_duplicate.len()))?;
    Ok(())
}
