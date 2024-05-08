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

use std::collections::{HashMap, HashSet};

use futures::StreamExt;
use itertools::Itertools;
use jj_lib::backend::{BackendError, BackendResult, CommitId, FileId, TreeValue};
use jj_lib::commit::{Commit, CommitIteratorExt};
use jj_lib::matchers::EverythingMatcher;
use jj_lib::merged_tree::MergedTreeBuilder;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::revset::{RevsetExpression, RevsetIteratorExt};
use jj_lib::store::Store;
use pollster::FutureExt;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Format files
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct FixArgs {
    /// Fix these commits and all their descendants
    #[arg(long, short)]
    sources: Vec<RevisionArg>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_fix(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &FixArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let root_commits: Vec<Commit> = workspace_command
        .parse_union_revsets(&args.sources)?
        .evaluate_to_commits()?
        .try_collect()?;
    workspace_command.check_rewritable(root_commits.iter().ids())?;

    let mut tx = workspace_command.start_transaction();

    // Collect all FileIds we're going to format and which commits they appear in
    let commits: Vec<_> = RevsetExpression::commits(root_commits.iter().ids().cloned().collect())
        .descendants()
        .evaluate_programmatic(tx.base_repo().as_ref())?
        .iter()
        .commits(tx.repo().store())
        .try_collect()?;
    let mut file_ids = HashSet::new();
    let mut commit_paths: HashMap<CommitId, Vec<RepoPathBuf>> = HashMap::new();
    for commit in commits.iter().rev() {
        let mut paths = vec![];
        // Paths modified in parent commits in the set should also be updated in this
        // commit
        for parent_id in commit.parent_ids() {
            if let Some(parent_paths) = commit_paths.get(parent_id) {
                paths.extend_from_slice(parent_paths);
            }
        }
        let parent_tree = commit.parent_tree(tx.repo())?;
        let tree = commit.tree()?;
        let mut diff_stream = parent_tree.diff_stream(&tree, &EverythingMatcher);
        async {
            while let Some((repo_path, diff)) = diff_stream.next().await {
                let (_before, after) = diff?;
                for term in after.into_iter().flatten() {
                    if let TreeValue::File { id, executable: _ } = term {
                        file_ids.insert((repo_path.clone(), id));
                        paths.push(repo_path.clone());
                    }
                }
            }
            Ok::<(), BackendError>(())
        }
        .block_on()?;
        commit_paths.insert(commit.id().clone(), paths);
    }

    let formatted = format_files(tx.repo().store().as_ref(), &file_ids)?;

    tx.mut_repo().transform_descendants(
        command.settings(),
        root_commits.iter().ids().cloned().collect_vec(),
        |mut rewriter| {
            let paths = commit_paths.get(rewriter.old_commit().id()).unwrap();
            let old_tree = rewriter.old_commit().tree()?;
            let mut tree_builder = MergedTreeBuilder::new(old_tree.id().clone());
            for path in paths {
                let old_value = old_tree.path_value(path);
                let new_value = old_value.map(|old_term| {
                    if let Some(TreeValue::File { id, executable }) = old_term {
                        if let Some(new_id) = formatted.get(&(path, id)) {
                            Some(TreeValue::File {
                                id: new_id.clone(),
                                executable: *executable,
                            })
                        } else {
                            old_term.clone()
                        }
                    } else {
                        old_term.clone()
                    }
                });
                if new_value != old_value {
                    tree_builder.set_or_remove(path.clone(), new_value);
                }
            }
            let new_tree = tree_builder.write_tree(rewriter.mut_repo().store())?;
            let builder = rewriter.reparent(command.settings())?;
            builder.set_tree_id(new_tree).write()?;
            Ok(())
        },
    )?;

    tx.finish(ui, format!("fixed {} commits", root_commits.len()))
}

fn format_files<'a>(
    store: &Store,
    file_ids: &'a HashSet<(RepoPathBuf, FileId)>,
) -> BackendResult<HashMap<(&'a RepoPathBuf, &'a FileId), FileId>> {
    let mut result = HashMap::new();
    for (path, id) in file_ids {
        // TODO: read asynchronously
        let mut read = store.read_file(path, id)?;
        let mut buf = vec![];
        read.read_to_end(&mut buf).unwrap();
        // TODO: Call a formatter instead of just uppercasing
        for b in &mut buf {
            b.make_ascii_uppercase();
        }
        // TODO: Don't write it if it didn't change
        let formatted_id = store.write_file(path, &mut buf.as_slice())?;
        result.insert((path, id), formatted_id);
    }
    Ok(result)
}
