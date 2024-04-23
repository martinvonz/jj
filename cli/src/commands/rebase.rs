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
use indexmap::IndexSet;
use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::{Commit, CommitIteratorExt};
use jj_lib::object_id::ObjectId;
use jj_lib::repo::{ReadonlyRepo, Repo};
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

    let target_commit_ids: HashSet<_> = target_commits.iter().ids().cloned().collect();

    // First, rebase the descendants of `target_commits`.
    let (target_roots, num_rebased_descendants) =
        extract_commits(&mut tx, settings, target_commits, &target_commit_ids)?;

    // We now update `new_parents` to account for the rebase of all of
    // `target_commits`'s descendants. Even if some of the original `new_parents`
    // were descendants of `target_commits`, this will no longer be the case after
    // the update.
    let new_parents = tx
        .mut_repo()
        .new_parents(new_parents.iter().ids().cloned().collect_vec());
    let mut skipped_commits = Vec::new();

    // At this point, all commits in the target set will only have other commits in
    // the set as their ancestors. We can now safely rebase `target_commits` onto
    // the `new_parents`, by updating the roots' parents and rebasing its
    // descendants.
    tx.mut_repo().transform_descendants(
        settings,
        target_roots.iter().cloned().collect_vec(),
        |mut rewriter| {
            let old_commit = rewriter.old_commit();
            let old_commit_id = old_commit.id().clone();

            if target_roots.contains(&old_commit_id) {
                rewriter.set_new_parents(new_parents.clone());
            }
            if rewriter.parents_changed() {
                rewriter.rebase(settings)?.write()?;
            } else {
                // Only include the commit in the list of skipped commits if it wasn't
                // previously rewritten. Commits in the target set could have previously been
                // rewritten in `extract_commits` if they are not a root, and some of its
                // parents are not part of the target set.
                if target_commit_ids.contains(&old_commit_id) {
                    skipped_commits.push(rewriter.old_commit().clone());
                }
            }
            Ok(())
        },
    )?;

    if let Some(mut fmt) = ui.status_formatter() {
        if !skipped_commits.is_empty() {
            log_skipped_rebase_commits_message(
                fmt.as_mut(),
                tx.base_workspace_helper(),
                skipped_commits.iter(),
            )?;
        }
        let num_rebased = target_commits.len() - skipped_commits.len();
        if num_rebased > 0 {
            writeln!(fmt, "Rebased {num_rebased} commits onto destination")?;
        }
        if num_rebased_descendants > 0 {
            writeln!(
                fmt,
                "Rebased {num_rebased_descendants} descendant commits onto parents of rebased \
                 commits"
            )?;
        }
    }
    if tx.mut_repo().has_changes() {
        tx.finish(ui, tx_description)
    } else {
        Ok(()) // Do not print "Nothing changed."
    }
}

/// Extracts `target_commits` from the graph by rebasing its descendants onto
/// its parents. This assumes that `target_commits` can be rewritten.
/// `target_commits` should be in reverse topological order.
/// Returns a tuple of the commit IDs of the roots of the `target_commits` set
/// and the number of rebased descendants which were not in the set.
fn extract_commits(
    tx: &mut WorkspaceCommandTransaction,
    settings: &UserSettings,
    target_commits: &[Commit],
    target_commit_ids: &HashSet<CommitId>,
) -> Result<(HashSet<CommitId>, usize), CommandError> {
    let connected_target_commits: Vec<_> =
        RevsetExpression::commits(target_commits.iter().ids().cloned().collect_vec())
            .connected()
            .evaluate_programmatic(tx.base_repo().as_ref())?
            .iter()
            .commits(tx.base_repo().store())
            .try_collect()?;

    // Commits in the target set should only have other commits in the set as
    // parents, except the roots of the set, which persist their original
    // parents.
    // If a commit in the set has a parent which is not in the set, but has
    // an ancestor which is in the set, then the commit will have that ancestor
    // as a parent.
    let mut new_target_parents: HashMap<CommitId, Vec<CommitId>> = HashMap::new();
    for commit in connected_target_commits.iter().rev() {
        // The roots of the set will not have any parents found in `new_target_parents`,
        // and will be stored in `new_target_parents` as an empty vector.
        let mut new_parents = vec![];
        for old_parent in commit.parent_ids() {
            if target_commit_ids.contains(old_parent) {
                new_parents.push(old_parent.clone());
            } else if let Some(parents) = new_target_parents.get(old_parent) {
                new_parents.extend(parents.iter().cloned());
            }
        }
        new_target_parents.insert(commit.id().clone(), new_parents);
    }
    new_target_parents.retain(|id, _| target_commit_ids.contains(id));

    // Compute the roots of `target_commits`.
    let target_roots: HashSet<_> = new_target_parents
        .iter()
        .filter(|(_, parents)| parents.is_empty())
        .map(|(commit_id, _)| commit_id.clone())
        .collect();

    // If a commit outside the target set has a commit in the target set as a
    // parent, then - after the transformation - it should have that commit's
    // ancestors which are not in the target set as parents.
    let mut new_child_parents: HashMap<CommitId, IndexSet<CommitId>> = HashMap::new();
    for commit in target_commits.iter().rev() {
        let mut new_parents = IndexSet::new();
        for old_parent in commit.parent_ids() {
            if let Some(parents) = new_child_parents.get(old_parent) {
                new_parents.extend(parents.iter().cloned());
            } else {
                new_parents.insert(old_parent.clone());
            }
        }
        new_child_parents.insert(commit.id().clone(), new_parents);
    }

    let mut num_rebased_descendants = 0;

    // TODO(ilyagr): Consider making it possible for these descendants
    // to become emptied, like --skip-empty. This would require writing careful
    // tests.
    tx.mut_repo().transform_descendants(
        settings,
        target_commits.iter().ids().cloned().collect_vec(),
        |mut rewriter| {
            let old_commit = rewriter.old_commit();
            let old_commit_id = old_commit.id().clone();

            // Commits in the target set should persist only rebased parents from the target
            // sets.
            if let Some(new_parents) = new_target_parents.get(&old_commit_id) {
                // If the commit does not have any parents in the target set, its parents
                // will be persisted since it is one of the roots of the set.
                if !new_parents.is_empty() {
                    rewriter.set_new_rewritten_parents(new_parents.clone());
                }
            }
            // Commits outside the target set should have references to commits inside the set
            // replaced.
            else if rewriter
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
                rewriter.rebase(settings)?.write()?;
                if !target_commit_ids.contains(&old_commit_id) {
                    num_rebased_descendants += 1;
                }
            }
            Ok(())
        },
    )?;

    Ok((target_roots, num_rebased_descendants))
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
