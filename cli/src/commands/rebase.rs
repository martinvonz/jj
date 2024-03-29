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

use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;

use clap::ArgGroup;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::{Commit, CommitIteratorExt};
use jj_lib::dag_walk;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::{MutableRepo, ReadonlyRepo, Repo};
use jj_lib::revset::{RevsetExpression, RevsetIteratorExt};
use jj_lib::rewrite::{rebase_commit_with_options, CommitRewriter, EmptyBehaviour, RebaseOptions};
use jj_lib::settings::UserSettings;
use tracing::instrument;

use crate::cli_util::{
    short_commit_hash, CommandHelper, RevisionArg, WorkspaceCommandHelper,
    WorkspaceCommandTransaction,
};
use crate::command_error::{user_error, CommandError};
use crate::formatter::Formatter;
use crate::ui::Ui;

/// Move revisions to different parent(s)
///
/// There are three different ways of specifying which revisions to rebase:
/// `-b` to rebase a whole branch, `-s` to rebase a revision and its
/// descendants, and `-r` to rebase a single commit. If none of them is
/// specified, it defaults to `-b @`.
///
/// With `-s`, the command rebases the specified revision and its descendants
/// onto the destination. For example, `jj rebase -s M -d O` would transform
/// your history like this (letters followed by an apostrophe are post-rebase
/// versions):
///
/// ```text
/// O           N'
/// |           |
/// | N         M'
/// | |         |
/// | M         O
/// | |    =>   |
/// | | L       | L
/// | |/        | |
/// | K         | K
/// |/          |/
/// J           J
/// ```
///
/// With `-b`, the command rebases the whole "branch" containing the specified
/// revision. A "branch" is the set of commits that includes:
///
/// * the specified revision and ancestors that are not also ancestors of the
///   destination
/// * all descendants of those commits
///
/// In other words, `jj rebase -b X -d Y` rebases commits in the revset
/// `(Y..X)::` (which is equivalent to `jj rebase -s 'roots(Y..X)' -d Y` for a
/// single root). For example, either `jj rebase -b L -d O` or `jj rebase -b M
/// -d O` would transform your history like this (because `L` and `M` are on the
/// same "branch", relative to the destination):
///
/// ```text
/// O           N'
/// |           |
/// | N         M'
/// | |         |
/// | M         | L'
/// | |    =>   |/
/// | | L       K'
/// | |/        |
/// | K         O
/// |/          |
/// J           J
/// ```
///
/// With `-r`, the command rebases only the specified revisions onto the
/// destination. Any "hole" left behind will be filled by rebasing descendants
/// onto the specified revision's parent(s). For example, `jj rebase -r K -d M`
/// would transform your history like this:
///
/// ```text
/// M          K'
/// |          |
/// | L        M
/// | |   =>   |
/// | K        | L'
/// |/         |/
/// J          J
/// ```
///
/// Note that you can create a merge commit by repeating the `-d` argument.
/// For example, if you realize that commit L actually depends on commit M in
/// order to work (in addition to its current parent K), you can run `jj rebase
/// -s L -d K -d M`:
///
/// ```text
/// M          L'
/// |          |\
/// | L        M |
/// | |   =>   | |
/// | K        | K
/// |/         |/
/// J          J
/// ```
///
/// If a working-copy commit gets abandoned, it will be given a new, empty
/// commit. This is true in general; it is not specific to this command.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
#[command(group(ArgGroup::new("to_rebase").args(&["branch", "source", "revisions"])))]
pub(crate) struct RebaseArgs {
    /// Rebase the whole branch relative to destination's ancestors (can be
    /// repeated)
    ///
    /// `jj rebase -b=br -d=dst` is equivalent to `jj rebase '-s=roots(dst..br)'
    /// -d=dst`.
    ///
    /// If none of `-b`, `-s`, or `-r` is provided, then the default is `-b @`.
    #[arg(long, short)]
    branch: Vec<RevisionArg>,

    /// Rebase specified revision(s) together with their trees of descendants
    /// (can be repeated)
    ///
    /// Each specified revision will become a direct child of the destination
    /// revision(s), even if some of the source revisions are descendants
    /// of others.
    ///
    /// If none of `-b`, `-s`, or `-r` is provided, then the default is `-b @`.
    #[arg(long, short)]
    source: Vec<RevisionArg>,
    /// Rebase the given revisions, rebasing descendants onto this revision's
    /// parent(s)
    ///
    /// Unlike `-s` or `-b`, you may `jj rebase -r` a revision `A` onto a
    /// descendant of `A`.
    ///
    /// If none of `-b`, `-s`, or `-r` is provided, then the default is `-b @`.
    #[arg(long, short)]
    revisions: Vec<RevisionArg>,
    /// The revision(s) to rebase onto (can be repeated to create a merge
    /// commit)
    #[arg(long, short, required = true)]
    destination: Vec<RevisionArg>,

    /// If true, when rebasing would produce an empty commit, the commit is
    /// abandoned. It will not be abandoned if it was already empty before the
    /// rebase. Will never skip merge commits with multiple non-empty
    /// parents.
    #[arg(long, conflicts_with = "revisions")]
    skip_empty: bool,

    /// Deprecated. Please prefix the revset with `all:` instead.
    #[arg(long, short = 'L', hide = true)]
    allow_large_revsets: bool,
}

#[instrument(skip_all)]
pub(crate) fn cmd_rebase(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &RebaseArgs,
) -> Result<(), CommandError> {
    if args.allow_large_revsets {
        return Err(user_error(
            "--allow-large-revsets has been deprecated.
Please use `jj rebase -d 'all:x|y'` instead of `jj rebase --allow-large-revsets -d x -d y`.",
        ));
    }

    let rebase_options = RebaseOptions {
        empty: match args.skip_empty {
            true => EmptyBehaviour::AbandonNewlyEmpty,
            false => EmptyBehaviour::Keep,
        },
        simplify_ancestor_merge: false,
    };
    let mut workspace_command = command.workspace_helper(ui)?;
    let new_parents = workspace_command
        .resolve_some_revsets_default_single(&args.destination)?
        .into_iter()
        .collect_vec();
    if !args.revisions.is_empty() {
        assert_eq!(
            // In principle, `-r --skip-empty` could mean to abandon the `-r`
            // commit if it becomes empty. This seems internally consistent with
            // the behavior of other commands, but is not very useful.
            //
            // It would become even more confusing once `-r --before` is
            // implemented. If `rebase -r` behaves like `abandon`, the
            // descendants of the `-r` commits should not be abandoned if
            // emptied. But it would also make sense for the descendants of the
            // `--before` commit to be abandoned if emptied. A commit can easily
            // be in both categories.
            rebase_options.empty,
            EmptyBehaviour::Keep,
            "clap should forbid `-r --skip-empty`"
        );
        let target_commits: Vec<_> = workspace_command
            .parse_union_revsets(&args.revisions)?
            .evaluate_to_commits()?
            .try_collect()?; // in reverse topological order
        rebase_revisions(
            ui,
            command.settings(),
            &mut workspace_command,
            &new_parents,
            &target_commits,
        )?;
    } else if !args.source.is_empty() {
        let source_commits = workspace_command.resolve_some_revsets_default_single(&args.source)?;
        rebase_descendants_transaction(
            ui,
            command.settings(),
            &mut workspace_command,
            new_parents,
            &source_commits,
            rebase_options,
        )?;
    } else {
        let branch_commits = if args.branch.is_empty() {
            IndexSet::from([workspace_command.resolve_single_rev(&RevisionArg::AT)?])
        } else {
            workspace_command.resolve_some_revsets_default_single(&args.branch)?
        };
        rebase_branch(
            ui,
            command.settings(),
            &mut workspace_command,
            new_parents,
            &branch_commits,
            rebase_options,
        )?;
    }
    Ok(())
}

fn rebase_branch(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: Vec<Commit>,
    branch_commits: &IndexSet<Commit>,
    rebase_options: RebaseOptions,
) -> Result<(), CommandError> {
    let parent_ids = new_parents
        .iter()
        .map(|commit| commit.id().clone())
        .collect_vec();
    let branch_commit_ids = branch_commits
        .iter()
        .map(|commit| commit.id().clone())
        .collect_vec();
    let roots_expression = RevsetExpression::commits(parent_ids)
        .range(&RevsetExpression::commits(branch_commit_ids))
        .roots();
    let root_commits: IndexSet<_> = roots_expression
        .evaluate_programmatic(workspace_command.repo().as_ref())
        .unwrap()
        .iter()
        .commits(workspace_command.repo().store())
        .try_collect()?;
    rebase_descendants_transaction(
        ui,
        settings,
        workspace_command,
        new_parents,
        &root_commits,
        rebase_options,
    )
}

/// Rebases `old_commits` onto `new_parents`.
fn rebase_descendants(
    tx: &mut WorkspaceCommandTransaction,
    settings: &UserSettings,
    new_parents: Vec<Commit>,
    old_commits: &[impl Borrow<Commit>],
    rebase_options: RebaseOptions,
) -> Result<usize, CommandError> {
    for old_commit in old_commits.iter() {
        let rewriter = CommitRewriter::new(
            tx.mut_repo(),
            old_commit.borrow().clone(),
            new_parents
                .iter()
                .map(|parent| parent.id().clone())
                .collect(),
        );
        rebase_commit_with_options(settings, rewriter, &rebase_options)?;
    }
    let num_rebased = old_commits.len()
        + tx.mut_repo()
            .rebase_descendants_with_options(settings, rebase_options)?;
    Ok(num_rebased)
}

fn rebase_descendants_transaction(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: Vec<Commit>,
    old_commits: &IndexSet<Commit>,
    rebase_options: RebaseOptions,
) -> Result<(), CommandError> {
    workspace_command.check_rewritable(old_commits.iter().ids())?;
    let (skipped_commits, old_commits) = old_commits
        .iter()
        .partition::<Vec<_>, _>(|commit| commit.parents() == new_parents);
    if !skipped_commits.is_empty() {
        if let Some(mut fmt) = ui.status_formatter() {
            log_skipped_rebase_commits_message(
                fmt.as_mut(),
                workspace_command,
                skipped_commits.into_iter(),
            )?;
        }
    }
    if old_commits.is_empty() {
        return Ok(());
    }
    for old_commit in old_commits.iter() {
        check_rebase_destinations(workspace_command.repo(), &new_parents, old_commit)?;
    }
    let mut tx = workspace_command.start_transaction();
    let num_rebased =
        rebase_descendants(&mut tx, settings, new_parents, &old_commits, rebase_options)?;
    writeln!(ui.status(), "Rebased {num_rebased} commits")?;
    let tx_message = if old_commits.len() == 1 {
        format!(
            "rebase commit {} and descendants",
            old_commits.first().unwrap().id().hex()
        )
    } else {
        format!("rebase {} commits and their descendants", old_commits.len())
    };
    tx.finish(ui, tx_message)?;
    Ok(())
}

fn rebase_revisions(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: &[Commit],
    target_commits: &[Commit],
) -> Result<(), CommandError> {
    if target_commits.is_empty() {
        return Ok(());
    }

    workspace_command.check_rewritable(target_commits.iter().ids())?;
    for commit in target_commits.iter() {
        if new_parents.contains(commit) {
            return Err(user_error(format!(
                "Cannot rebase {} onto itself",
                short_commit_hash(commit.id()),
            )));
        }
    }

    move_commits_transaction(
        ui,
        settings,
        workspace_command,
        &new_parents.iter().ids().cloned().collect_vec(),
        &[],
        target_commits,
    )
}

/// Wraps `move_commits` in a transaction.
fn move_commits_transaction(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parent_ids: &[CommitId],
    new_children: &[Commit],
    target_commits: &[Commit],
) -> Result<(), CommandError> {
    if target_commits.is_empty() {
        return Ok(());
    }

    let mut tx = workspace_command.start_transaction();
    let tx_description = if target_commits.len() == 1 {
        format!("rebase commit {}", target_commits[0].id().hex())
    } else {
        format!(
            "rebase commit {} and {} more",
            target_commits[0].id().hex(),
            target_commits.len() - 1
        )
    };

    let (num_rebased_targets, num_rebased_descendants, skipped_commits) = move_commits(
        settings,
        tx.mut_repo(),
        new_parent_ids,
        new_children,
        target_commits,
    )?;

    if let Some(mut fmt) = ui.status_formatter() {
        if !skipped_commits.is_empty() {
            log_skipped_rebase_commits_message(
                fmt.as_mut(),
                tx.base_workspace_helper(),
                skipped_commits.iter(),
            )?;
        }
        if num_rebased_targets > 0 {
            writeln!(
                fmt,
                "Rebased {num_rebased_targets} commits onto destination"
            )?;
        }
        if num_rebased_descendants > 0 {
            writeln!(fmt, "Rebased {num_rebased_descendants} descendant commits")?;
        }
    }

    tx.finish(ui, tx_description)
}

/// Moves `target_commits` from their current location to a new location in the
/// graph, given by the set of `new_parent_ids` and `new_children`.
/// The roots of `target_commits` are rebased onto the new parents, while the
/// new children are rebased onto the heads of `target_commits`.
/// This assumes that `target_commits` and `new_children` can be rewritten, and
/// there will be no cycles in the resulting graph.
/// `target_commits` should be in reverse topological order.
fn move_commits(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
    new_parent_ids: &[CommitId],
    new_children: &[Commit],
    target_commits: &[Commit],
) -> Result<(usize, usize, Vec<Commit>), CommandError> {
    if target_commits.is_empty() {
        return Ok((0, 0, vec![]));
    }

    let target_commit_ids: HashSet<_> = target_commits.iter().ids().cloned().collect();

    let connected_target_commits: Vec<_> =
        RevsetExpression::commits(target_commits.iter().ids().cloned().collect_vec())
            .connected()
            .evaluate_programmatic(mut_repo)?
            .iter()
            .commits(mut_repo.store())
            .try_collect()?;

    // Commits in the target set should only have other commits in the set as
    // parents, except the roots of the set, which persist their original
    // parents.
    // If a commit in the set has a parent which is not in the set, but has
    // an ancestor which is in the set, then the commit will have that ancestor
    // as a parent.
    let mut target_commits_internal_parents: HashMap<CommitId, Vec<CommitId>> = HashMap::new();
    for commit in connected_target_commits.iter().rev() {
        // The roots of the set will not have any parents found in `new_target_parents`,
        // and will be stored in `new_target_parents` as an empty vector.
        let mut new_parents = vec![];
        for old_parent in commit.parent_ids() {
            if target_commit_ids.contains(old_parent) {
                new_parents.push(old_parent.clone());
            } else if let Some(parents) = target_commits_internal_parents.get(old_parent) {
                new_parents.extend(parents.iter().cloned());
            }
        }
        target_commits_internal_parents.insert(commit.id().clone(), new_parents);
    }
    target_commits_internal_parents.retain(|id, _| target_commit_ids.contains(id));

    // Compute the roots of `target_commits`.
    let target_roots: HashSet<_> = target_commits_internal_parents
        .iter()
        .filter(|(_, parents)| parents.is_empty())
        .map(|(commit_id, _)| commit_id.clone())
        .collect();

    // If a commit outside the target set has a commit in the target set as a
    // parent, then - after the transformation - it should have that commit's
    // ancestors which are not in the target set as parents.
    let mut target_commits_external_parents: HashMap<CommitId, IndexSet<CommitId>> = HashMap::new();
    for commit in target_commits.iter().rev() {
        let mut new_parents = IndexSet::new();
        for old_parent in commit.parent_ids() {
            if let Some(parents) = target_commits_external_parents.get(old_parent) {
                new_parents.extend(parents.iter().cloned());
            } else {
                new_parents.insert(old_parent.clone());
            }
        }
        target_commits_external_parents.insert(commit.id().clone(), new_parents);
    }

    // If the new parents include a commit in the target set, replace it with the
    // commit's ancestors which are outside the set.
    // e.g. `jj rebase -r A --before A`
    let new_parent_ids: Vec<_> = new_parent_ids
        .iter()
        .flat_map(|parent_id| {
            if let Some(parent_ids) = target_commits_external_parents.get(parent_id) {
                parent_ids.iter().cloned().collect_vec()
            } else {
                [parent_id.clone()].to_vec()
            }
        })
        .collect();

    // If the new children include a commit in the target set, replace it with the
    // commit's descendants which are outside the set.
    // e.g. `jj rebase -r A --after A`
    let new_children: Vec<_> = if new_children
        .iter()
        .any(|child| target_commit_ids.contains(child.id()))
    {
        let target_commits_descendants: Vec<_> =
            RevsetExpression::commits(target_commit_ids.iter().cloned().collect_vec())
                .union(
                    &RevsetExpression::commits(target_commit_ids.iter().cloned().collect_vec())
                        .children(),
                )
                .evaluate_programmatic(mut_repo)?
                .iter()
                .commits(mut_repo.store())
                .try_collect()?;

        // For all commits in the target set, compute its transitive descendant commits
        // which are outside of the target set by up to 1 generation.
        let mut target_commit_external_descendants: HashMap<CommitId, IndexSet<Commit>> =
            HashMap::new();
        // Iterate through all descendants of the target set, going through children
        // before parents.
        for commit in target_commits_descendants.iter() {
            if !target_commit_external_descendants.contains_key(commit.id()) {
                let children = if target_commit_ids.contains(commit.id()) {
                    IndexSet::new()
                } else {
                    IndexSet::from([commit.clone()])
                };
                target_commit_external_descendants.insert(commit.id().clone(), children);
            }

            let children = target_commit_external_descendants
                .get(commit.id())
                .unwrap()
                .iter()
                .cloned()
                .collect_vec();
            for parent_id in commit.parent_ids() {
                if target_commit_ids.contains(parent_id) {
                    if let Some(target_children) =
                        target_commit_external_descendants.get_mut(parent_id)
                    {
                        target_children.extend(children.iter().cloned());
                    } else {
                        target_commit_external_descendants
                            .insert(parent_id.clone(), children.iter().cloned().collect());
                    }
                };
            }
        }

        new_children
            .iter()
            .flat_map(|child| {
                if let Some(children) = target_commit_external_descendants.get(child.id()) {
                    children.iter().cloned().collect_vec()
                } else {
                    [child.clone()].to_vec()
                }
            })
            .collect()
    } else {
        new_children.to_vec()
    };

    // Compute the parents of the new children, which will include the heads of the
    // target set.
    let new_children_parents: HashMap<_, _> = if !new_children.is_empty() {
        // Compute the heads of the target set, which will be used as the parents of
        // `new_children`.
        let mut target_heads: HashSet<CommitId> = HashSet::new();
        for commit in connected_target_commits.iter().rev() {
            target_heads.insert(commit.id().clone());
            for old_parent in commit.parent_ids() {
                target_heads.remove(old_parent);
            }
        }
        let target_heads = connected_target_commits
            .iter()
            .rev()
            .filter(|commit| {
                target_heads.contains(commit.id()) && target_commit_ids.contains(commit.id())
            })
            .map(|commit| commit.id().clone())
            .collect_vec();

        new_children
            .iter()
            .map(|child_commit| {
                let mut new_child_parent_ids: IndexSet<_> = child_commit
                    .parent_ids()
                    .iter()
                    // Replace target commits with their parents outside the target set.
                    .flat_map(|id| {
                        if let Some(parents) = target_commits_external_parents.get(id) {
                            parents.iter().cloned().collect_vec()
                        } else {
                            [id.clone()].to_vec()
                        }
                    })
                    // Exclude any of the new parents of the target commits, since we are
                    // "inserting" the target commits in between the new parents and the new
                    // children.
                    .filter(|id| {
                        !new_parent_ids
                            .iter()
                            .any(|new_parent_id| new_parent_id == id)
                    })
                    .collect();

                // Add `target_heads` as parents of the new child commit.
                new_child_parent_ids.extend(target_heads.clone());

                (
                    child_commit.id().clone(),
                    new_child_parent_ids.iter().cloned().collect_vec(),
                )
            })
            .collect()
    } else {
        HashMap::new()
    };

    // Compute the set of commits to visit, which includes the target commits, the
    // new children commits (if any), and their descendants.
    let mut roots = target_roots.iter().cloned().collect_vec();
    roots.extend(new_children.iter().ids().cloned());
    let to_visit_expression = RevsetExpression::commits(roots).descendants();
    let to_visit: Vec<_> = to_visit_expression
        .evaluate_programmatic(mut_repo)?
        .iter()
        .commits(mut_repo.store())
        .try_collect()?;
    let to_visit_commits: IndexMap<_, _> = to_visit
        .into_iter()
        .map(|commit| (commit.id().clone(), commit))
        .collect();

    let to_visit_commits_new_parents: HashMap<_, _> = to_visit_commits
        .iter()
        .map(|(commit_id, commit)| {
            let new_parents =
            // New child of the rebased target commits.
            if let Some(new_child_parents) = new_children_parents.get(commit_id) {
                new_child_parents.clone()
            }
            // Commits in the target set should persist only rebased parents from the target
            // sets.
            else if let Some(target_commit_parents) =
                target_commits_internal_parents.get(commit_id)
            {
                // If the commit does not have any parents in the target set, it is one of the
                // commits in the root set, and should be rebased onto the new destination.
                if target_commit_parents.is_empty() {
                    new_parent_ids.clone()
                } else {
                    target_commit_parents.clone()
                }
            }
            // Commits outside the target set should have references to commits inside the set
            // replaced.
            else if commit
                .parent_ids()
                .iter()
                .any(|id| target_commits_external_parents.contains_key(id))
            {
                let mut new_parents = vec![];
                for parent in commit.parent_ids() {
                    if let Some(parents) = target_commits_external_parents.get(parent) {
                        new_parents.extend(parents.iter().cloned());
                    } else {
                        new_parents.push(parent.clone());
                    }
                }
                new_parents
            } else {
                commit.parent_ids().iter().cloned().collect_vec()
            };

            (commit_id.clone(), new_parents)
        })
        .collect();

    // Re-compute the order of commits to visit, such that each commit's new parents
    // must be visited first.
    let mut visited: HashSet<CommitId> = HashSet::new();
    let mut to_visit = dag_walk::topo_order_reverse(
        to_visit_commits.keys().cloned().collect_vec(),
        |commit_id| commit_id.clone(),
        |commit_id| -> Vec<CommitId> {
            visited.insert(commit_id.clone());
            to_visit_commits_new_parents
                .get(commit_id)
                .cloned()
                .unwrap()
                .iter()
                // Only add parents which are in the set to be visited and have not already been
                // visited.
                .filter(|&id| to_visit_commits.contains_key(id) && !visited.contains(id))
                .cloned()
                .collect()
        },
    );

    let mut skipped_commits: Vec<Commit> = vec![];
    let mut num_rebased_targets = 0;
    let mut num_rebased_descendants = 0;

    // Rebase each commit onto its new parents in the reverse topological order
    // computed above.
    // TODO(ilyagr): Consider making it possible for descendants of the target set
    // to become emptied, like --skip-empty. This would require writing careful
    // tests.
    while let Some(old_commit_id) = to_visit.pop() {
        let old_commit = to_visit_commits.get(&old_commit_id).unwrap();
        let parent_ids = to_visit_commits_new_parents
            .get(&old_commit_id)
            .cloned()
            .unwrap();
        let new_parent_ids = mut_repo.new_parents(parent_ids);
        let rewriter = CommitRewriter::new(mut_repo, old_commit.clone(), new_parent_ids);
        if rewriter.parents_changed() {
            rewriter.rebase(settings)?.write()?;
            if target_commit_ids.contains(&old_commit_id) {
                num_rebased_targets += 1;
            } else {
                num_rebased_descendants += 1;
            }
        } else {
            skipped_commits.push(old_commit.clone());
        }
    }
    mut_repo.update_rewritten_references(settings)?;

    Ok((
        num_rebased_targets,
        num_rebased_descendants,
        skipped_commits,
    ))
}

fn check_rebase_destinations(
    repo: &Arc<ReadonlyRepo>,
    new_parents: &[Commit],
    commit: &Commit,
) -> Result<(), CommandError> {
    for parent in new_parents {
        if repo.index().is_ancestor(commit.id(), parent.id()) {
            return Err(user_error(format!(
                "Cannot rebase {} onto descendant {}",
                short_commit_hash(commit.id()),
                short_commit_hash(parent.id())
            )));
        }
    }
    Ok(())
}

fn log_skipped_rebase_commits_message<'a>(
    fmt: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    commits: impl ExactSizeIterator<Item = &'a Commit>,
) -> Result<(), CommandError> {
    let template = workspace_command.commit_summary_template();
    if commits.len() == 1 {
        write!(fmt, "Skipping rebase of commit ")?;
        template.format(commits.into_iter().next().unwrap(), fmt)?;
        writeln!(fmt)?;
    } else {
        writeln!(fmt, "Skipping rebase of commits:")?;
        for commit in commits {
            write!(fmt, "  ")?;
            template.format(commit, fmt)?;
            writeln!(fmt)?;
        }
    }
    Ok(())
}
