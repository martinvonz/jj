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

use std::io::Write;
use std::rc::Rc;
use std::sync::Arc;

use clap::ArgGroup;
use clap_complete::ArgValueCandidates;
use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::commit::CommitIteratorExt;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::repo::Repo;
use jj_lib::revset::ResolvedRevsetExpression;
use jj_lib::revset::RevsetExpression;
use jj_lib::revset::RevsetIteratorExt;
use jj_lib::rewrite::move_commits;
use jj_lib::rewrite::EmptyBehaviour;
use jj_lib::rewrite::MoveCommitsStats;
use jj_lib::rewrite::MoveCommitsTarget;
use jj_lib::rewrite::RebaseOptions;
use jj_lib::settings::UserSettings;
use tracing::instrument;

use crate::cli_util::short_commit_hash;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::cli_error;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::complete;
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
    #[arg(long, short, add = ArgValueCandidates::new(complete::mutable_revisions))]
    branch: Vec<RevisionArg>,

    /// Rebase specified revision(s) together with their trees of descendants
    /// (can be repeated)
    ///
    /// Each specified revision will become a direct child of the destination
    /// revision(s), even if some of the source revisions are descendants
    /// of others.
    ///
    /// If none of `-b`, `-s`, or `-r` is provided, then the default is `-b @`.
    #[arg(long, short, add = ArgValueCandidates::new(complete::mutable_revisions))]
    source: Vec<RevisionArg>,
    /// Rebase the given revisions, rebasing descendants onto this revision's
    /// parent(s)
    ///
    /// Unlike `-s` or `-b`, you may `jj rebase -r` a revision `A` onto a
    /// descendant of `A`.
    ///
    /// If none of `-b`, `-s`, or `-r` is provided, then the default is `-b @`.
    #[arg(long, short, add = ArgValueCandidates::new(complete::mutable_revisions))]
    revisions: Vec<RevisionArg>,

    #[command(flatten)]
    destination: RebaseDestinationArgs,

    /// Deprecated. Use --skip-emptied instead.
    #[arg(long, conflicts_with = "revisions", hide = true)]
    skip_empty: bool,

    /// If true, when rebasing would produce an empty commit, the commit is
    /// abandoned. It will not be abandoned if it was already empty before the
    /// rebase. Will never skip merge commits with multiple non-empty
    /// parents.
    #[arg(long)]
    skip_emptied: bool,
}

#[derive(clap::Args, Clone, Debug)]
#[group(required = true)]
pub struct RebaseDestinationArgs {
    /// The revision(s) to rebase onto (can be repeated to create a merge
    /// commit)
    #[arg(long, short, add = ArgValueCandidates::new(complete::all_revisions))]
    destination: Option<Vec<RevisionArg>>,
    /// The revision(s) to insert after (can be repeated to create a merge
    /// commit)
    #[arg(
        long,
        short = 'A',
        visible_alias = "after",
        conflicts_with = "destination",
        add = ArgValueCandidates::new(complete::all_revisions),
    )]
    insert_after: Option<Vec<RevisionArg>>,
    /// The revision(s) to insert before (can be repeated to create a merge
    /// commit)
    #[arg(
        long,
        short = 'B',
        visible_alias = "before",
        conflicts_with = "destination",
        add = ArgValueCandidates::new(complete::mutable_revisions),
    )]
    insert_before: Option<Vec<RevisionArg>>,
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
        rebase_revisions(
            ui,
            command.settings(),
            &mut workspace_command,
            &args.revisions,
            &args.destination,
            &rebase_options,
        )?;
    } else if !args.source.is_empty() {
        rebase_source(
            ui,
            command.settings(),
            &mut workspace_command,
            &args.source,
            &args.destination,
            &rebase_options,
        )?;
    } else {
        rebase_branch(
            ui,
            command.settings(),
            &mut workspace_command,
            &args.branch,
            &args.destination,
            rebase_options,
        )?;
    }
    Ok(())
}

fn rebase_revisions(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    revisions: &[RevisionArg],
    rebase_destination: &RebaseDestinationArgs,
    rebase_options: &RebaseOptions,
) -> Result<(), CommandError> {
    let target_commits: Vec<_> = workspace_command
        .parse_union_revsets(ui, revisions)?
        .evaluate_to_commits()?
        .try_collect()?; // in reverse topological order
    workspace_command.check_rewritable(target_commits.iter().ids())?;

    let (new_parents, new_children) =
        compute_rebase_destination(ui, workspace_command, rebase_destination)?;
    if rebase_destination.destination.is_some() && new_children.is_empty() {
        for commit in &target_commits {
            if new_parents.contains(commit) {
                return Err(user_error(format!(
                    "Cannot rebase {} onto itself",
                    short_commit_hash(commit.id()),
                )));
            }
        }
    }
    rebase_revisions_transaction(
        ui,
        settings,
        workspace_command,
        &new_parents.iter().ids().cloned().collect_vec(),
        &new_children,
        target_commits,
        rebase_options,
    )
}

fn rebase_source(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    source: &[RevisionArg],
    rebase_destination: &RebaseDestinationArgs,
    rebase_options: &RebaseOptions,
) -> Result<(), CommandError> {
    let source_commits = workspace_command
        .resolve_some_revsets_default_single(ui, source)?
        .into_iter()
        .collect_vec();
    workspace_command.check_rewritable(source_commits.iter().ids())?;

    let (new_parents, new_children) =
        compute_rebase_destination(ui, workspace_command, rebase_destination)?;
    if rebase_destination.destination.is_some() && new_children.is_empty() {
        for commit in &source_commits {
            check_rebase_destinations(workspace_command.repo(), &new_parents, commit)?;
        }
    }

    rebase_descendants_transaction(
        ui,
        settings,
        workspace_command,
        &new_parents.iter().ids().cloned().collect_vec(),
        &new_children,
        source_commits,
        rebase_options,
    )
}

fn rebase_branch(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    branch: &[RevisionArg],
    rebase_destination: &RebaseDestinationArgs,
    rebase_options: RebaseOptions,
) -> Result<(), CommandError> {
    let branch_commits: Vec<_> = if branch.is_empty() {
        vec![workspace_command.resolve_single_rev(ui, &RevisionArg::AT)?]
    } else {
        workspace_command
            .resolve_some_revsets_default_single(ui, branch)?
            .iter()
            .cloned()
            .collect_vec()
    };

    let (new_parents, new_children) =
        compute_rebase_destination(ui, workspace_command, rebase_destination)?;
    let new_parent_ids = new_parents.iter().ids().cloned().collect_vec();
    let branch_commit_ids = branch_commits.iter().ids().cloned().collect_vec();
    let roots_expression = RevsetExpression::commits(new_parent_ids.clone())
        .range(&RevsetExpression::commits(branch_commit_ids))
        .roots();
    let root_commits: Vec<_> = roots_expression
        .evaluate(workspace_command.repo().as_ref())
        .unwrap()
        .iter()
        .commits(workspace_command.repo().store())
        .try_collect()?;
    workspace_command.check_rewritable(root_commits.iter().ids())?;
    if rebase_destination.destination.is_some() && new_children.is_empty() {
        for commit in &root_commits {
            check_rebase_destinations(workspace_command.repo(), &new_parents, commit)?;
        }
    }

    rebase_descendants_transaction(
        ui,
        settings,
        workspace_command,
        &new_parent_ids,
        &new_children,
        root_commits,
        &rebase_options,
    )
}

fn rebase_descendants_transaction(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parent_ids: &[CommitId],
    new_children: &[Commit],
    target_roots: Vec<Commit>,
    rebase_options: &RebaseOptions,
) -> Result<(), CommandError> {
    if target_roots.is_empty() {
        return Ok(());
    }

    let mut tx = workspace_command.start_transaction();
    let tx_description = if target_roots.len() == 1 {
        format!(
            "rebase commit {} and descendants",
            target_roots.first().unwrap().id().hex()
        )
    } else {
        format!(
            "rebase {} commits and their descendants",
            target_roots.len()
        )
    };

    let stats = move_commits(
        settings,
        tx.repo_mut(),
        new_parent_ids,
        new_children,
        &MoveCommitsTarget::Roots(target_roots),
        rebase_options,
    )?;
    print_move_commits_stats(ui, &stats)?;
    tx.finish(ui, tx_description)
}

/// Computes the new parents and children for the given
/// [`RebaseDestinationArgs`].
fn compute_rebase_destination(
    ui: &mut Ui,
    workspace_command: &mut WorkspaceCommandHelper,
    rebase_destination: &RebaseDestinationArgs,
) -> Result<(Vec<Commit>, Vec<Commit>), CommandError> {
    let resolve_revisions =
        |revisions: &Option<Vec<RevisionArg>>| -> Result<Option<Vec<Commit>>, CommandError> {
            if let Some(revisions) = revisions {
                Ok(Some(
                    workspace_command
                        .resolve_some_revsets_default_single(ui, revisions)?
                        .into_iter()
                        .collect_vec(),
                ))
            } else {
                Ok(None)
            }
        };
    let destination_commits = resolve_revisions(&rebase_destination.destination)?;
    let after_commits = resolve_revisions(&rebase_destination.insert_after)?;
    let before_commits = resolve_revisions(&rebase_destination.insert_before)?;

    let (new_parents, new_children) = match (destination_commits, after_commits, before_commits) {
        (Some(destination_commits), None, None) => (destination_commits, vec![]),
        (None, Some(after_commits), Some(before_commits)) => (after_commits, before_commits),
        (None, Some(after_commits), None) => {
            let new_children: Vec<_> =
                RevsetExpression::commits(after_commits.iter().ids().cloned().collect_vec())
                    .children()
                    .evaluate(workspace_command.repo().as_ref())?
                    .iter()
                    .commits(workspace_command.repo().store())
                    .try_collect()?;

            (after_commits, new_children)
        }
        (None, None, Some(before_commits)) => {
            // Not using `RevsetExpression::parents` here to persist the order of parents
            // specified in `before_commits`.
            let new_parent_ids = before_commits
                .iter()
                .flat_map(|commit| commit.parent_ids().iter().cloned().collect_vec())
                .unique()
                .collect_vec();
            let new_parents: Vec<_> = new_parent_ids
                .iter()
                .map(|commit_id| workspace_command.repo().store().get_commit(commit_id))
                .try_collect()?;

            (new_parents, before_commits)
        }
        _ => unreachable!(),
    };

    if !new_children.is_empty() {
        workspace_command.check_rewritable(new_children.iter().ids())?;
        ensure_no_commit_loop(
            workspace_command.repo().as_ref(),
            &RevsetExpression::commits(new_children.iter().ids().cloned().collect_vec()),
            &RevsetExpression::commits(new_parents.iter().ids().cloned().collect_vec()),
        )?;
    }

    Ok((new_parents, new_children))
}

/// Creates a transaction for rebasing revisions.
fn rebase_revisions_transaction(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parent_ids: &[CommitId],
    new_children: &[Commit],
    target_commits: Vec<Commit>,
    rebase_options: &RebaseOptions,
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

    let stats = move_commits(
        settings,
        tx.repo_mut(),
        new_parent_ids,
        new_children,
        &MoveCommitsTarget::Commits(target_commits),
        rebase_options,
    )?;
    print_move_commits_stats(ui, &stats)?;
    tx.finish(ui, tx_description)
}

/// Ensure that there is no possible cycle between the potential children and
/// parents of rebased commits.
fn ensure_no_commit_loop(
    repo: &ReadonlyRepo,
    children_expression: &Rc<ResolvedRevsetExpression>,
    parents_expression: &Rc<ResolvedRevsetExpression>,
) -> Result<(), CommandError> {
    if let Some(commit_id) = children_expression
        .dag_range_to(parents_expression)
        .evaluate(repo)?
        .iter()
        .next()
    {
        let commit_id = commit_id?;
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

/// Print details about the provided [`MoveCommitsStats`].
fn print_move_commits_stats(ui: &Ui, stats: &MoveCommitsStats) -> std::io::Result<()> {
    let Some(mut formatter) = ui.status_formatter() else {
        return Ok(());
    };
    let &MoveCommitsStats {
        num_rebased_targets,
        num_rebased_descendants,
        num_skipped_rebases,
        num_abandoned,
    } = stats;
    if num_skipped_rebases > 0 {
        writeln!(
            formatter,
            "Skipped rebase of {num_skipped_rebases} commits that were already in place"
        )?;
    }
    if num_rebased_targets > 0 {
        writeln!(
            formatter,
            "Rebased {num_rebased_targets} commits onto destination"
        )?;
    }
    if num_rebased_descendants > 0 {
        writeln!(
            formatter,
            "Rebased {num_rebased_descendants} descendant commits"
        )?;
    }
    if num_abandoned > 0 {
        writeln!(formatter, "Abandoned {num_abandoned} newly emptied commits")?;
    }
    Ok(())
}
