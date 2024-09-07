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
use std::io::Write;
use std::rc::Rc;
use std::sync::Arc;

use clap::ArgGroup;
use indexmap::IndexSet;
use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::commit::CommitIteratorExt;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::repo::Repo;
use jj_lib::revset::RevsetExpression;
use jj_lib::revset::RevsetIteratorExt;
use jj_lib::rewrite::move_commits;
use jj_lib::rewrite::rebase_commit_with_options;
use jj_lib::rewrite::CommitRewriter;
use jj_lib::rewrite::EmptyBehaviour;
use jj_lib::rewrite::MoveCommitsStats;
use jj_lib::rewrite::RebaseOptions;
use jj_lib::settings::UserSettings;
use tracing::instrument;

use crate::cli_util::short_commit_hash;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::cli_util::WorkspaceCommandHelper;
use crate::cli_util::WorkspaceCommandTransaction;
use crate::command_error::cli_error;
use crate::command_error::user_error;
use crate::command_error::CommandError;
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
#[command(group(ArgGroup::new("target").args(&["destination", "insert_after", "insert_before"]).multiple(true).required(true)))]
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
    #[arg(long, short)]
    destination: Vec<RevisionArg>,
    /// The revision(s) to insert after (can be repeated to create a merge
    /// commit)
    ///
    /// Only works with `-r`.
    #[arg(
        long,
        short = 'A',
        visible_alias = "after",
        conflicts_with = "destination",
        conflicts_with = "source",
        conflicts_with = "branch"
    )]
    insert_after: Vec<RevisionArg>,
    /// The revision(s) to insert before (can be repeated to create a merge
    /// commit)
    ///
    /// Only works with `-r`.
    #[arg(
        long,
        short = 'B',
        visible_alias = "before",
        conflicts_with = "destination",
        conflicts_with = "source",
        conflicts_with = "branch"
    )]
    insert_before: Vec<RevisionArg>,

    /// Deprecated. Use --skip-emptied instead.
    #[arg(long, conflicts_with = "revisions", hide = true)]
    skip_empty: bool,

    /// If true, when rebasing would produce an empty commit, the commit is
    /// abandoned. It will not be abandoned if it was already empty before the
    /// rebase. Will never skip merge commits with multiple non-empty
    /// parents.
    #[arg(long, conflicts_with = "revisions")]
    skip_emptied: bool,
}

#[instrument(skip_all)]
pub(crate) fn cmd_rebase(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &RebaseArgs,
) -> Result<(), CommandError> {
    if args.skip_empty {
        return Err(cli_error(
            "--skip-empty is deprecated, and has been renamed to --skip-emptied.",
        ));
    }

    let rebase_options = RebaseOptions {
        empty: match args.skip_emptied {
            true => EmptyBehaviour::AbandonNewlyEmpty,
            false => EmptyBehaviour::Keep,
        },
        simplify_ancestor_merge: false,
    };
    let mut workspace_command = command.workspace_helper(ui)?;
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
        if !args.insert_after.is_empty() && !args.insert_before.is_empty() {
            let after_commits =
                workspace_command.resolve_some_revsets_default_single(&args.insert_after)?;
            let before_commits =
                workspace_command.resolve_some_revsets_default_single(&args.insert_before)?;
            rebase_revisions_after_before(
                ui,
                command.settings(),
                &mut workspace_command,
                &after_commits,
                &before_commits,
                &target_commits,
            )?;
        } else if !args.insert_after.is_empty() {
            let after_commits =
                workspace_command.resolve_some_revsets_default_single(&args.insert_after)?;
            rebase_revisions_after(
                ui,
                command.settings(),
                &mut workspace_command,
                &after_commits,
                &target_commits,
            )?;
        } else if !args.insert_before.is_empty() {
            let before_commits =
                workspace_command.resolve_some_revsets_default_single(&args.insert_before)?;
            rebase_revisions_before(
                ui,
                command.settings(),
                &mut workspace_command,
                &before_commits,
                &target_commits,
            )?;
        } else {
            let new_parents = workspace_command
                .resolve_some_revsets_default_single(&args.destination)?
                .into_iter()
                .collect_vec();
            rebase_revisions(
                ui,
                command.settings(),
                &mut workspace_command,
                &new_parents,
                &target_commits,
            )?;
        }
    } else if !args.source.is_empty() {
        let new_parents = workspace_command
            .resolve_some_revsets_default_single(&args.destination)?
            .into_iter()
            .collect_vec();
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
        let new_parents = workspace_command
            .resolve_some_revsets_default_single(&args.destination)?
            .into_iter()
            .collect_vec();
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
            tx.repo_mut(),
            old_commit.borrow().clone(),
            new_parents
                .iter()
                .map(|parent| parent.id().clone())
                .collect(),
        );
        rebase_commit_with_options(settings, rewriter, &rebase_options)?;
    }
    let num_rebased = old_commits.len()
        + tx.repo_mut()
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
        .partition::<Vec<_>, _>(|commit| commit.parent_ids().iter().eq(new_parents.iter().ids()));
    let num_skipped_rebases = skipped_commits.len();
    if num_skipped_rebases > 0 {
        writeln!(
            ui.status(),
            "Skipped rebase of {num_skipped_rebases} commits that were already in place"
        )?;
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

fn rebase_revisions_after(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    after_commits: &IndexSet<Commit>,
    target_commits: &[Commit],
) -> Result<(), CommandError> {
    workspace_command.check_rewritable(target_commits.iter().ids())?;

    let after_commit_ids = after_commits.iter().ids().cloned().collect_vec();
    let new_parents_expression = RevsetExpression::commits(after_commit_ids.clone());
    let new_children_expression = new_parents_expression.children();

    ensure_no_commit_loop(
        workspace_command.repo().as_ref(),
        &new_children_expression,
        &new_parents_expression,
    )?;

    let new_parent_ids = after_commit_ids;
    let new_children: Vec<_> = new_children_expression
        .evaluate_programmatic(workspace_command.repo().as_ref())?
        .iter()
        .commits(workspace_command.repo().store())
        .try_collect()?;
    workspace_command.check_rewritable(new_children.iter().ids())?;

    move_commits_transaction(
        ui,
        settings,
        workspace_command,
        &new_parent_ids,
        &new_children,
        target_commits,
    )
}

fn rebase_revisions_before(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    before_commits: &IndexSet<Commit>,
    target_commits: &[Commit],
) -> Result<(), CommandError> {
    workspace_command.check_rewritable(target_commits.iter().ids())?;
    let before_commit_ids = before_commits.iter().ids().cloned().collect_vec();
    workspace_command.check_rewritable(&before_commit_ids)?;

    let new_children_expression = RevsetExpression::commits(before_commit_ids);
    let new_parents_expression = new_children_expression.parents();

    ensure_no_commit_loop(
        workspace_command.repo().as_ref(),
        &new_children_expression,
        &new_parents_expression,
    )?;

    // Not using `new_parents_expression` here to persist the order of parents
    // specified in `before_commits`.
    let new_parent_ids: IndexSet<_> = before_commits
        .iter()
        .flat_map(|commit| commit.parent_ids().iter().cloned().collect_vec())
        .collect();
    let new_parent_ids = new_parent_ids.into_iter().collect_vec();
    let new_children = before_commits.iter().cloned().collect_vec();

    move_commits_transaction(
        ui,
        settings,
        workspace_command,
        &new_parent_ids,
        &new_children,
        target_commits,
    )
}

fn rebase_revisions_after_before(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    after_commits: &IndexSet<Commit>,
    before_commits: &IndexSet<Commit>,
    target_commits: &[Commit],
) -> Result<(), CommandError> {
    workspace_command.check_rewritable(target_commits.iter().ids())?;
    let before_commit_ids = before_commits.iter().ids().cloned().collect_vec();
    workspace_command.check_rewritable(&before_commit_ids)?;

    let after_commit_ids = after_commits.iter().ids().cloned().collect_vec();
    let new_children_expression = RevsetExpression::commits(before_commit_ids);
    let new_parents_expression = RevsetExpression::commits(after_commit_ids.clone());

    ensure_no_commit_loop(
        workspace_command.repo().as_ref(),
        &new_children_expression,
        &new_parents_expression,
    )?;

    let new_parent_ids = after_commit_ids;
    let new_children = before_commits.iter().cloned().collect_vec();

    move_commits_transaction(
        ui,
        settings,
        workspace_command,
        &new_parent_ids,
        &new_children,
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

    let MoveCommitsStats {
        num_rebased_targets,
        num_rebased_descendants,
        num_skipped_rebases,
    } = move_commits(
        settings,
        tx.repo_mut(),
        new_parent_ids,
        new_children,
        target_commits,
    )?;

    if let Some(mut fmt) = ui.status_formatter() {
        if num_skipped_rebases > 0 {
            writeln!(
                fmt,
                "Skipped rebase of {num_skipped_rebases} commits that were already in place"
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

/// Ensure that there is no possible cycle between the potential children and
/// parents of rebased commits.
fn ensure_no_commit_loop(
    repo: &ReadonlyRepo,
    children_expression: &Rc<RevsetExpression>,
    parents_expression: &Rc<RevsetExpression>,
) -> Result<(), CommandError> {
    if let Some(commit_id) = children_expression
        .dag_range_to(parents_expression)
        .evaluate_programmatic(repo)?
        .iter()
        .next()
    {
        return Err(user_error(format!(
            "Refusing to create a loop: commit {} would be both an ancestor and a descendant of \
             the rebased commits",
            short_commit_hash(&commit_id),
        )));
    }
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
