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

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use clap::ArgGroup;
use indexmap::IndexSet;
use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::revset::{RevsetExpression, RevsetIteratorExt};
use jj_lib::rewrite::{rebase_commit, rebase_commit_with_options, EmptyBehaviour, RebaseOptions};
use jj_lib::settings::UserSettings;
use tracing::instrument;

use crate::cli_util::{
    resolve_multiple_nonempty_revsets_default_single, short_commit_hash, CommandHelper,
    RevisionArg, WorkspaceCommandHelper,
};
use crate::command_error::{user_error, CommandError};
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
/// With `-r`, the command rebases only the specified revision onto the
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
#[command(group(ArgGroup::new("to_rebase").args(&["branch", "source", "revision"])))]
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

    /// Rebase specified revision(s) together their tree of descendants (can be
    /// repeated)
    ///
    /// Each specified revision will become a direct child of the destination
    /// revision(s), even if some of the source revisions are descendants
    /// of others.
    ///
    /// If none of `-b`, `-s`, or `-r` is provided, then the default is `-b @`.
    #[arg(long, short)]
    source: Vec<RevisionArg>,
    /// Rebase only this revision, rebasing descendants onto this revision's
    /// parent(s)
    ///
    /// Unlike `-s` or `-b`, you may `jj rebase -r` a revision `A` onto a
    /// descendant of `A`.
    ///
    /// If none of `-b`, `-s`, or `-r` is provided, then the default is `-b @`.
    #[arg(long, short)]
    revision: Option<RevisionArg>,
    /// The revision(s) to rebase onto (can be repeated to create a merge
    /// commit)
    #[arg(long, short, required = true)]
    destination: Vec<RevisionArg>,

    /// If true, when rebasing would produce an empty commit, the commit is
    /// abandoned. It will not be abandoned if it was already empty before the
    /// rebase. Will never skip merge commits with multiple non-empty
    /// parents.
    #[arg(long, conflicts_with = "revision")]
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
    let new_parents =
        resolve_multiple_nonempty_revsets_default_single(&workspace_command, &args.destination)?
            .into_iter()
            .collect_vec();
    if let Some(rev_str) = &args.revision {
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
        rebase_revision(
            ui,
            command.settings(),
            &mut workspace_command,
            &new_parents,
            rev_str,
        )?;
    } else if !args.source.is_empty() {
        let source_commits =
            resolve_multiple_nonempty_revsets_default_single(&workspace_command, &args.source)?;
        rebase_descendants(
            ui,
            command.settings(),
            &mut workspace_command,
            &new_parents,
            &source_commits,
            rebase_options,
        )?;
    } else {
        let branch_commits = if args.branch.is_empty() {
            IndexSet::from([workspace_command.resolve_single_rev("@")?])
        } else {
            resolve_multiple_nonempty_revsets_default_single(&workspace_command, &args.branch)?
        };
        rebase_branch(
            ui,
            command.settings(),
            &mut workspace_command,
            &new_parents,
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
    new_parents: &[Commit],
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
    rebase_descendants(
        ui,
        settings,
        workspace_command,
        new_parents,
        &root_commits,
        rebase_options,
    )
}

fn rebase_descendants(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: &[Commit],
    old_commits: &IndexSet<Commit>,
    rebase_options: RebaseOptions,
) -> Result<(), CommandError> {
    workspace_command.check_rewritable(old_commits)?;
    let (skipped_commits, old_commits) = old_commits
        .iter()
        .partition::<Vec<_>, _>(|commit| commit.parents() == new_parents);
    if !skipped_commits.is_empty() {
        log_skipped_rebase_commits_message(ui, workspace_command, &skipped_commits)?;
    }
    if old_commits.is_empty() {
        return Ok(());
    }
    for old_commit in old_commits.iter() {
        check_rebase_destinations(workspace_command.repo(), new_parents, old_commit)?;
    }
    let mut tx = workspace_command.start_transaction();
    // `rebase_descendants` takes care of sorting in reverse topological order, so
    // no need to do it here.
    for old_commit in old_commits.iter() {
        rebase_commit_with_options(
            settings,
            tx.mut_repo(),
            old_commit,
            new_parents,
            &rebase_options,
        )?;
    }
    let num_rebased = old_commits.len()
        + tx.mut_repo()
            .rebase_descendants_with_options(settings, rebase_options)?;
    writeln!(ui.stderr(), "Rebased {num_rebased} commits")?;
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

fn rebase_revision(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: &[Commit],
    rev_str: &str,
) -> Result<(), CommandError> {
    let old_commit = workspace_command.resolve_single_rev(rev_str)?;
    workspace_command.check_rewritable([&old_commit])?;
    if new_parents.contains(&old_commit) {
        return Err(user_error(format!(
            "Cannot rebase {} onto itself",
            short_commit_hash(old_commit.id()),
        )));
    }

    let children_expression = RevsetExpression::commit(old_commit.id().clone()).children();
    let child_commits: Vec<_> = children_expression
        .evaluate_programmatic(workspace_command.repo().as_ref())
        .unwrap()
        .iter()
        .commits(workspace_command.repo().store())
        .try_collect()?;
    // Currently, immutable commits are defined so that a child of a rewriteable
    // commit is always rewriteable.
    debug_assert!(workspace_command.check_rewritable(&child_commits).is_ok());

    // First, rebase the children of `old_commit`.
    let mut tx = workspace_command.start_transaction();
    let mut rebased_commit_ids = HashMap::new();
    for child_commit in &child_commits {
        let new_child_parent_ids: Vec<CommitId> = child_commit
            .parents()
            .iter()
            .flat_map(|c| {
                if c == &old_commit {
                    old_commit
                        .parents()
                        .iter()
                        .map(|c| c.id().clone())
                        .collect()
                } else {
                    [c.id().clone()].to_vec()
                }
            })
            .collect();

        // Some of the new parents may be ancestors of others as in
        // `test_rebase_single_revision`.
        let new_child_parents_expression = RevsetExpression::commits(new_child_parent_ids.clone())
            .minus(
                &RevsetExpression::commits(new_child_parent_ids.clone())
                    .parents()
                    .ancestors(),
            );
        let new_child_parents: Vec<Commit> = new_child_parents_expression
            .evaluate_programmatic(tx.base_repo().as_ref())
            .unwrap()
            .iter()
            .commits(tx.base_repo().store())
            .try_collect()?;

        rebased_commit_ids.insert(
            child_commit.id().clone(),
            rebase_commit(settings, tx.mut_repo(), child_commit, &new_child_parents)?
                .id()
                .clone(),
        );
    }
    // Now, rebase the descendants of the children.
    // TODO(ilyagr): Consider making it possible for these descendants to become
    // emptied, like --skip_empty. This would require writing careful tests.
    rebased_commit_ids.extend(tx.mut_repo().rebase_descendants_return_map(settings)?);
    let num_rebased_descendants = rebased_commit_ids.len();

    // We now update `new_parents` to account for the rebase of all of
    // `old_commit`'s descendants. Even if some of the original `new_parents`
    // were descendants of `old_commit`, this will no longer be the case after
    // the update.
    //
    // To make the update simpler, we assume that each commit was rewritten only
    // once; we don't have a situation where both `(A,B)` and `(B,C)` are in
    // `rebased_commit_ids`.
    //
    // TODO(BUG #2650): There is something wrong with this assumption, the next TODO
    // seems to be a little optimistic. See the panicked test in
    // `test_rebase_with_child_and_descendant_bug_2600`.
    //
    // TODO(ilyagr): This assumption relies on the fact that, after
    // `rebase_descendants`, a descendant of `old_commit` cannot also be a
    // direct child of `old_commit`. This fact will likely change, see
    // https://github.com/martinvonz/jj/issues/2600. So, the code needs to be
    // updated before that happens. This would also affect
    // `test_rebase_with_child_and_descendant_bug_2600`.
    //
    // The issue is that if a child and a descendant of `old_commit` were the
    // same commit (call it `Q`), it would be rebased first by `rebase_commit`
    // above, and then the result would be rebased again by
    // `rebase_descendants_return_map`. Then, if we were trying to rebase
    // `old_commit` onto `Q`, new_parents would only account for one of these.
    let new_parents: Vec<_> = new_parents
        .iter()
        .map(|new_parent| {
            rebased_commit_ids
                .get(new_parent.id())
                .map_or(Ok(new_parent.clone()), |rebased_new_parent_id| {
                    tx.repo().store().get_commit(rebased_new_parent_id)
                })
        })
        .try_collect()?;

    // Finally, it's safe to rebase `old_commit`. We can skip rebasing if it is
    // already a child of `new_parents`. Otherwise, at this point, it should no
    // longer have any children; they have all been rebased and the originals
    // have been abandoned.
    let skipped_commit_rebase = if old_commit.parents() == new_parents {
        write!(ui.stderr(), "Skipping rebase of commit ")?;
        tx.write_commit_summary(ui.stderr_formatter().as_mut(), &old_commit)?;
        writeln!(ui.stderr())?;
        true
    } else {
        rebase_commit(settings, tx.mut_repo(), &old_commit, &new_parents)?;
        debug_assert_eq!(tx.mut_repo().rebase_descendants(settings)?, 0);
        false
    };

    if num_rebased_descendants > 0 {
        if skipped_commit_rebase {
            writeln!(
                ui.stderr(),
                "Rebased {num_rebased_descendants} descendant commits onto parent of commit"
            )?;
        } else {
            writeln!(
                ui.stderr(),
                "Also rebased {num_rebased_descendants} descendant commits onto parent of rebased \
                 commit"
            )?;
        }
    }
    if tx.mut_repo().has_changes() {
        tx.finish(ui, format!("rebase commit {}", old_commit.id().hex()))
    } else {
        Ok(()) // Do not print "Nothing changed."
    }
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

fn log_skipped_rebase_commits_message(
    ui: &Ui,
    workspace_command: &WorkspaceCommandHelper,
    commits: &[&Commit],
) -> Result<(), CommandError> {
    let mut fmt = ui.stderr_formatter();
    let template = workspace_command.commit_summary_template();
    if commits.len() == 1 {
        write!(fmt, "Skipping rebase of commit ")?;
        template.format(commits[0], fmt.as_mut())?;
        writeln!(fmt)?;
    } else {
        writeln!(fmt, "Skipping rebase of commits:")?;
        for commit in commits {
            write!(fmt, "  ")?;
            template.format(commit, fmt.as_mut())?;
            writeln!(fmt)?;
        }
    }
    Ok(())
}
