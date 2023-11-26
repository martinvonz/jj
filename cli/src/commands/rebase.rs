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
use jj_lib::backend::{CommitId, ObjectId};
use jj_lib::commit::Commit;
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::revset::{RevsetExpression, RevsetIteratorExt};
use jj_lib::rewrite::{rebase_commit, rebase_commit_with_options, EmptyBehaviour, RebaseOptions};
use jj_lib::settings::UserSettings;
use tracing::instrument;

use crate::cli_util::{
    self, resolve_multiple_nonempty_revsets_default_single, short_commit_hash, user_error,
    CommandError, CommandHelper, RevisionArg, WorkspaceCommandHelper,
};
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
///
/// With `-r`, the command rebases only the specified revision onto the
/// destination. Any "hole" left behind will be filled by rebasing descendants
/// onto the specified revision's parent(s). For example, `jj rebase -r K -d M`
/// would transform your history like this:
///
/// M          K'
/// |          |
/// | L        M
/// | |   =>   |
/// | K        | L'
/// |/         |/
/// J          J
///
/// Note that you can create a merge commit by repeating the `-d` argument.
/// For example, if you realize that commit L actually depends on commit M in
/// order to work (in addition to its current parent K), you can run `jj rebase
/// -s L -d K -d M`:
///
/// M          L'
/// |          |\
/// | L        M |
/// | |   =>   | |
/// | K        | K
/// |/         |/
/// J          J
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
    /// skipped.
    /// Will never skip merge commits with multiple non-empty parents.
    #[arg(long)]
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
            true => EmptyBehaviour::AbandonAllEmpty,
            false => EmptyBehaviour::Keep,
        },
    };
    let mut workspace_command = command.workspace_helper(ui)?;
    let new_parents = cli_util::resolve_all_revs(&workspace_command, ui, &args.destination)?
        .into_iter()
        .collect_vec();
    if let Some(rev_str) = &args.revision {
        rebase_revision(
            ui,
            command.settings(),
            &mut workspace_command,
            &new_parents,
            rev_str,
            &rebase_options,
        )?;
    } else if !args.source.is_empty() {
        let source_commits =
            resolve_multiple_nonempty_revsets_default_single(&workspace_command, ui, &args.source)?;
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
            IndexSet::from([workspace_command.resolve_single_rev("@", ui)?])
        } else {
            resolve_multiple_nonempty_revsets_default_single(&workspace_command, ui, &args.branch)?
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
    for old_commit in old_commits.iter() {
        check_rebase_destinations(workspace_command.repo(), new_parents, old_commit)?;
    }
    let tx_message = if old_commits.len() == 1 {
        format!(
            "rebase commit {} and descendants",
            old_commits.first().unwrap().id().hex()
        )
    } else {
        format!("rebase {} commits and their descendants", old_commits.len())
    };
    let mut tx = workspace_command.start_transaction(&tx_message);
    // `rebase_descendants` takes care of sorting in reverse topological order, so
    // no need to do it here.
    for old_commit in old_commits {
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
    tx.finish(ui)?;
    Ok(())
}

fn rebase_revision(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: &[Commit],
    rev_str: &str,
    rebase_options: &RebaseOptions,
) -> Result<(), CommandError> {
    let old_commit = workspace_command.resolve_single_rev(rev_str, ui)?;
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
    // Currently, immutable commits are defied so that a child of a rewriteable
    // commit is always rewriteable.
    debug_assert!(workspace_command.check_rewritable(&child_commits).is_ok());

    // First, rebase the children of `old_commit`.
    let mut tx =
        workspace_command.start_transaction(&format!("rebase commit {}", old_commit.id().hex()));
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
    // `old_commit`'s descendants. Even if some of the original `new_parents` were
    // descendants of `old_commit`, this will no longer be the case after the
    // update.
    //
    // To make the update simpler, we assume that each commit was rewritten only
    // once; we don't have a situation where both `(A,B)` and `(B,C)` are in
    // `rebased_commit_ids`. This assumption relies on the fact that a descendant of
    // a child of `old_commit` cannot also be a direct child of `old_commit`.
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

    // Finally, it's safe to rebase `old_commit`. At this point, it should no longer
    // have any children; they have all been rebased and the originals have been
    // abandoned.
    rebase_commit_with_options(
        settings,
        tx.mut_repo(),
        &old_commit,
        &new_parents,
        rebase_options,
    )?;
    debug_assert_eq!(tx.mut_repo().rebase_descendants(settings)?, 0);

    if num_rebased_descendants > 0 {
        writeln!(
            ui.stderr(),
            "Also rebased {num_rebased_descendants} descendant commits onto parent of rebased \
             commit"
        )?;
    }
    tx.finish(ui)?;
    Ok(())
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
