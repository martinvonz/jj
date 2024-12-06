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

use clap_complete::ArgValueCandidates;
use itertools::Itertools;
use jj_lib::commit::CommitIteratorExt;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

use super::rebase::compute_rebase_destination;
use super::rebase::RebaseDestinationArgs;

#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct SerializeArgs {
    /// Revisions to serialize
    #[arg(add = ArgValueCandidates::new(complete::mutable_revisions))]
    revisions: Vec<RevisionArg>,

    #[command(flatten)]
    destination: RebaseDestinationArgs,
}

#[instrument(skip_all)]
pub(crate) fn cmd_serialize(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SerializeArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let mut target_commits: Vec<_> = args.revisions.iter().rev().map(
        |revset| -> Result<_, CommandError> {
            let evaluator = workspace_command.parse_revset(ui, revset)?;
            let commits: Vec<_> = evaluator.evaluate_to_commits()?.try_collect()?;
            Ok(commits)
        }
    ).flatten_ok().try_collect()?;

    target_commits = target_commits.into_iter().unique().collect();

    workspace_command.check_rewritable(target_commits.iter().ids())?;

    let (mut new_parents, mut new_children) =
        compute_rebase_destination(ui, &mut workspace_command, &args.destination)?;

    // TODO(algmyr): Lookup into vector is inefficient, can we do better?
    new_parents.retain(|c| !target_commits.contains(c));
    new_children.retain(|c| !target_commits.contains(c));

    let mut parent: HashMap<_, _> = target_commits
        .iter()
        .tuple_windows()
        .map(|(c, p)| (c.id().clone(), p.id().clone()))
        .collect();

    // Commits should be in (child, ..., parent) order.

    for p in &new_parents {
        if let Some(c) = target_commits.last() {
            parent.insert(c.id().clone(), p.id().clone());
        }
    }
    for c in &new_children {
        if let Some(p) = target_commits.first() {
            parent.insert(c.id().clone(), p.id().clone());
        }
    }
    target_commits.extend(new_children);

    let mut tx = workspace_command.start_transaction();

    tx.repo_mut().transform_descendants(
        command.settings(),
        target_commits.iter().ids().cloned().collect_vec(),
        |mut rewriter| {
            // Commits in the target set do not depend on each other but they still depend
            // on other parents
            if let Some(new_parent) = parent.get(rewriter.old_commit().id()) {
                rewriter.set_new_rewritten_parents(&[new_parent.clone()]);
            }
            if rewriter.parents_changed() {
                let builder = rewriter.rebase(command.settings())?;
                builder.write()?;
            }
            Ok(())
        },
    )?;

    tx.finish(ui, format!("serialize {} commits", target_commits.len()))
}
