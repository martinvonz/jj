// Copyright 2024 The Jujutsu Authors
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

use indexmap::IndexSet;
use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::commit::CommitIteratorExt;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Parallelize revisions by making them siblings
///
/// Running `jj parallelize 1::2` will transform the history like this:
/// ```text
/// 3
/// |             3
/// 2            / \
/// |    ->     1   2
/// 1            \ /
/// |             0
/// 0
/// ```
///
/// The command effectively says "these revisions are actually independent",
/// meaning that they should no longer be ancestors/descendants of each other.
/// However, revisions outside the set that were previously ancestors of a
/// revision in the set will remain ancestors of it. For example, revision 0
/// above remains an ancestor of both 1 and 2. Similarly,
/// revisions outside the set that were previously descendants of a revision
/// in the set will remain descendants of it. For example, revision 3 above
/// remains a descendant of both 1 and 2.
///
/// Therefore, `jj parallelize '1 | 3'` is a no-op. That's because 2, which is
/// not in the target set, was a descendant of 1 before, so it remains a
/// descendant, and it was an ancestor of 3 before, so it remains an ancestor.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct ParallelizeArgs {
    /// Revisions to parallelize
    revisions: Vec<RevisionArg>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_parallelize(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ParallelizeArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    // The target commits are the commits being parallelized. They are ordered
    // here with children before parents.
    let target_commits: Vec<Commit> = workspace_command
        .parse_union_revsets(&args.revisions)?
        .evaluate_to_commits()?
        .try_collect()?;
    workspace_command.check_rewritable(target_commits.iter().ids())?;

    let mut tx = workspace_command.start_transaction();

    // New parents for commits in the target set. Since commits in the set are now
    // supposed to be independent, they inherit the parent's non-target parents,
    // recursively.
    let mut new_target_parents: HashMap<CommitId, Vec<CommitId>> = HashMap::new();
    for commit in target_commits.iter().rev() {
        let mut new_parents = vec![];
        for old_parent in commit.parent_ids() {
            if let Some(grand_parents) = new_target_parents.get(old_parent) {
                new_parents.extend_from_slice(grand_parents);
            } else {
                new_parents.push(old_parent.clone());
            }
        }
        new_target_parents.insert(commit.id().clone(), new_parents);
    }

    // If a commit outside the target set has a commit in the target set as parent,
    // then - after the transformation - it should also have that commit's
    // parents as direct parents, if those commits are also in the target set.
    let mut new_child_parents: HashMap<CommitId, IndexSet<CommitId>> = HashMap::new();
    for commit in target_commits.iter().rev() {
        let mut new_parents = IndexSet::new();
        for old_parent in commit.parent_ids() {
            if let Some(parents) = new_child_parents.get(old_parent) {
                new_parents.extend(parents.iter().cloned());
            }
        }
        new_parents.insert(commit.id().clone());
        new_child_parents.insert(commit.id().clone(), new_parents);
    }

    tx.mut_repo().transform_descendants(
        command.settings(),
        target_commits.iter().ids().cloned().collect_vec(),
        |mut rewriter| {
            // Commits in the target set do not depend on each other but they still depend
            // on other parents
            if let Some(new_parents) = new_target_parents.get(rewriter.old_commit().id()) {
                rewriter.set_new_rewritten_parents(new_parents.clone());
            } else if rewriter
                .old_commit()
                .parent_ids()
                .iter()
                .any(|id| new_child_parents.contains_key(id))
            {
                let mut new_parents = vec![];
                for parent in rewriter.old_commit().parent_ids() {
                    if let Some(parents) = new_child_parents.get(parent) {
                        new_parents.extend(parents.iter().cloned());
                    } else {
                        new_parents.push(parent.clone());
                    }
                }
                rewriter.set_new_rewritten_parents(new_parents);
            }
            if rewriter.parents_changed() {
                let builder = rewriter.rebase(command.settings())?;
                builder.write()?;
            }
            Ok(())
        },
    )?;

    tx.finish(ui, format!("parallelize {} commits", target_commits.len()))
}
