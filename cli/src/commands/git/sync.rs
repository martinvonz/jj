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

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;

use clap_complete::ArgValueCandidates;
use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::op_store::RemoteRefState;
use jj_lib::repo::Repo;
use jj_lib::revset::FailingSymbolResolver;
use jj_lib::revset::RevsetExpression;
use jj_lib::revset::RevsetIteratorExt;
use jj_lib::rewrite::EmptyBehaviour;
use jj_lib::str_util::StringPattern;

use crate::cli_util::short_commit_hash;
use crate::cli_util::CommandHelper;
use crate::cli_util::WorkspaceCommandTransaction;
use crate::command_error::CommandError;
use crate::complete;
use crate::git_util::get_fetch_remotes;
use crate::git_util::get_git_repo;
use crate::git_util::git_fetch;
use crate::git_util::FetchArgs;
use crate::ui::Ui;

/// Sync the local `jj` repo to remote Git branch(es).
///
/// The sync command will first fetch from the Git remote(s), then
/// rebase all local changes onto the appropriate updated
/// heads that were fetched.
///
/// Changes that are made empty by the rebase are dropped.
#[derive(clap::Args, Clone, Debug)]
pub struct GitSyncArgs {
    /// Rebase the specified branches only.
    ///
    /// Note that this affects only the rebase behaviour, as
    /// the fetch behaviour always fetches all branches.
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// expand `*` as a glob. The other wildcard characters aren't supported.
    #[arg(long, short,
          alias="bookmark",
          default_value = "glob:*",
          value_parser = StringPattern::parse,
          add = ArgValueCandidates::new(complete::bookmarks),
    )]
    pub branch: Vec<StringPattern>,
    /// Fetch from all remotes
    ///
    /// By default, the fetch will only use remotes configured in the
    /// `git.fetch` section of the config.
    ///
    /// When specified, --all-remotes causes the fetch to use all remotes known
    /// to the underlying git repo.
    #[arg(long, default_value = "false")]
    pub all_remotes: bool,
}

pub fn cmd_git_sync(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitSyncArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let mut tx = workspace_command.start_transaction();

    let guard = tracing::debug_span!("git.sync.pre-fetch").entered();
    let prefetch_heads = get_bookmark_heads(tx.base_repo().as_ref(), &args.branch)?;
    let candidates = CandidateCommit::get(tx.repo(), &prefetch_heads)?;
    drop(guard);

    let guard = tracing::debug_span!("git.sync.fetch").entered();
    git_fetch_all(ui, &mut tx, args.all_remotes)?;
    drop(guard);

    let guard = tracing::debug_span!("git.sync.post-fetch").entered();
    let postfetch_heads = get_bookmark_heads(tx.repo(), &args.branch)?;
    let update_record = UpdateRecord::new(
        &tx,
        &BranchHeads {
            prefetch: &prefetch_heads,
            postfetch: &postfetch_heads,
        },
    );
    drop(guard);

    let guard = tracing::debug_span!("git.sync.rebase").entered();
    let settings = tx.settings().clone();
    let mut num_rebased = 0;

    tx.repo_mut().transform_descendants(
        &settings,
        update_record.get_rebase_roots(&candidates),
        |mut rewriter| {
            rewriter.simplify_ancestor_merge();
            let mut updated_parents: Vec<CommitId> = vec![];

            let old_parents = rewriter.new_parents().iter().cloned().collect_vec();

            let old_commit = short_commit_hash(rewriter.old_commit().id());
            for parent in &old_parents {
                let old = short_commit_hash(parent);
                if let Some(updated) = update_record.maybe_update_commit(rewriter.repo(), parent) {
                    let new = short_commit_hash(&updated);
                    tracing::debug!("rebase {old_commit} from {old} to {new}");
                    updated_parents.push(updated.clone());
                } else {
                    tracing::debug!("not rebasing {old_commit} from {old}");
                    updated_parents.push(parent.clone());
                }
            }

            rewriter.set_new_parents(updated_parents);

            if let Some(builder) =
                rewriter.rebase_with_empty_behavior(&settings, EmptyBehaviour::AbandonNewlyEmpty)?
            {
                builder.write()?;
                num_rebased += 1;
            }

            Ok(())
        },
    )?;

    tx.finish(
        ui,
        format!("sync completed; {num_rebased} commits rebased to new heads"),
    )?;

    drop(guard);

    Ok(())
}

/// Returns a vector of commit ids corresponding to the target commit
/// of local bookmarks matching the supplied patterns.
fn get_bookmark_heads(
    repo: &dyn Repo,
    bookmarks: &[StringPattern],
) -> Result<Vec<CommitId>, CommandError> {
    let mut commits: Vec<CommitId> = vec![];
    let local_bookmarks = bookmarks
        .iter()
        .flat_map(|pattern| {
            repo.view()
                .local_bookmarks_matching(pattern)
                .map(|(name, _ref_target)| name)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    for bookmark in local_bookmarks {
        tracing::debug!("fetching heads for bookmark {bookmark}");
        let bookmark_commits: Vec<Commit> =
            RevsetExpression::bookmarks(StringPattern::exact(bookmark.to_string()))
                .resolve_user_expression(repo, &FailingSymbolResolver)?
                .evaluate(repo)?
                .iter()
                .commits(repo.store())
                .try_collect()?;

        commits.append(
            &mut bookmark_commits
                .iter()
                .map(|commit| commit.id().clone())
                .collect::<Vec<_>>(),
        );
        tracing::debug!("..Ok");
    }

    Ok(commits)
}

fn set_diff(lhs: &[CommitId], rhs: &[CommitId]) -> Vec<CommitId> {
    BTreeSet::from_iter(lhs.to_vec())
        .difference(&BTreeSet::from_iter(rhs.to_vec()))
        .cloned()
        .collect_vec()
}

struct BranchHeads<'a> {
    prefetch: &'a [CommitId],
    postfetch: &'a [CommitId],
}

struct UpdateRecord {
    old_to_new: BTreeMap<CommitId, CommitId>,
}

impl UpdateRecord {
    fn new(tx: &WorkspaceCommandTransaction, heads: &BranchHeads) -> Self {
        let new_heads = set_diff(heads.postfetch, heads.prefetch);
        let needs_rebase = set_diff(heads.prefetch, heads.postfetch);

        let mut old_to_new: BTreeMap<CommitId, CommitId> = BTreeMap::from([]);

        for new in &new_heads {
            for old in &needs_rebase {
                if old != new && tx.repo().index().is_ancestor(old, new) {
                    old_to_new.insert(old.clone(), new.clone());
                }
            }
        }

        for (k, v) in &old_to_new {
            let old = short_commit_hash(k);
            let new = short_commit_hash(v);
            tracing::debug!("rebase children of {old} to {new}");
        }

        UpdateRecord { old_to_new }
    }

    /// Returns commits that need to be rebased.
    ///
    /// The returned commits all have parents in the `old_to_new` mapping, which
    /// means that the branch their parents belong to, have advanced to new
    /// commits.
    fn get_rebase_roots(&self, candidates: &[CandidateCommit]) -> Vec<CommitId> {
        candidates
            .iter()
            .filter_map(|candidate| {
                if self.old_to_new.contains_key(&candidate.parent) {
                    Some(candidate.child.clone())
                } else {
                    None
                }
            })
            .collect_vec()
    }

    fn maybe_update_commit(&self, repo: &dyn Repo, commit: &CommitId) -> Option<CommitId> {
        self.old_to_new
            .values()
            .filter_map(|new| {
                if new != commit && repo.index().is_ancestor(commit, new) {
                    Some(new.clone())
                } else {
                    None
                }
            })
            .next()
    }
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
pub struct CandidateCommit {
    parent: CommitId,
    child: CommitId,
}

impl fmt::Display for CandidateCommit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let parent = short_commit_hash(&self.parent);
        let child = short_commit_hash(&self.child);
        write!(f, "=> {parent} --> {child}")
    }
}

impl CandidateCommit {
    fn get(repo: &dyn Repo, start: &[CommitId]) -> Result<Vec<CandidateCommit>, CommandError> {
        let commits: Vec<Commit> = RevsetExpression::commits(start.to_vec())
            .descendants()
            .minus(&RevsetExpression::remote_bookmarks(
                StringPattern::everything(),
                StringPattern::everything(),
                Some(RemoteRefState::New),
            ))
            .resolve_user_expression(repo, &FailingSymbolResolver)?
            .evaluate(repo)?
            .iter()
            .commits(repo.store())
            .try_collect()?;

        Ok(commits
            .iter()
            .flat_map(|commit| {
                commit
                    .parent_ids()
                    .iter()
                    .map(|parent_id| {
                        let candidate = CandidateCommit {
                            parent: parent_id.clone(),
                            child: commit.id().clone(),
                        };
                        tracing::debug!("candidate: {candidate}");
                        candidate
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>())
    }
}

fn git_fetch_all(
    ui: &mut Ui,
    tx: &mut WorkspaceCommandTransaction,
    use_all_remotes: bool,
) -> Result<(), CommandError> {
    let git_repo = get_git_repo(tx.base_repo().store())?;
    let remotes = get_fetch_remotes(ui, tx.settings(), &git_repo, &[], use_all_remotes)?;

    tracing::debug!("fetching from remotes: {}", remotes.join(","));

    git_fetch(
        ui,
        tx,
        &git_repo,
        &FetchArgs {
            branch: &[StringPattern::everything()],
            remotes: &remotes,
        },
    )
}
