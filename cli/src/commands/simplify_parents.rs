use std::collections::HashSet;

use clap_complete::ArgValueCandidates;
use itertools::Itertools;
use jj_lib::revset::RevsetExpression;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Simplify parent edges for the specified revision(s).
///
/// Removes all parents of each of the specified revisions that are also
/// indirect ancestors of the same revisions through other parents. This has no
/// effect on any revision's contents, including the working copy.
///
/// In other words, for all (A, B, C) where A has (B, C) as parents and C is an
/// ancestor of B, A will be rewritten to have only B as a parent instead of
/// B+C.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct SimplifyParentsArgs {
    /// Simplify specified revision(s) together with their trees of descendants
    /// (can be repeated)
    #[arg(long, short, add = ArgValueCandidates::new(complete::mutable_revisions))]
    source: Vec<RevisionArg>,
    /// Simplify specified revision(s) (can be repeated)
    ///
    /// If both `--source` and `--revisions` are not provided, this defaults to
    /// the `revsets.simplify-parents` setting, or `reachable(@, mutable())`
    /// if it is not set.
    #[arg(long, short, add = ArgValueCandidates::new(complete::mutable_revisions))]
    revisions: Vec<RevisionArg>,
}

pub(crate) fn cmd_simplify_parents(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SimplifyParentsArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let revs = if args.source.is_empty() && args.revisions.is_empty() {
        let revs = command.settings().get_string("revsets.simplify-parents")?;
        workspace_command
            .parse_revset(ui, &RevisionArg::from(revs))?
            .expression()
            .clone()
    } else {
        RevsetExpression::descendants(
            workspace_command
                .parse_union_revsets(ui, &args.source)?
                .expression(),
        )
        .union(
            workspace_command
                .parse_union_revsets(ui, &args.revisions)?
                .expression(),
        )
    };
    let commit_ids: Vec<_> = workspace_command
        .attach_revset_evaluator(revs)
        .evaluate_to_commit_ids()?
        .try_collect()?;
    workspace_command.check_rewritable(&commit_ids)?;
    let commit_ids_set: HashSet<_> = commit_ids.iter().cloned().collect();
    let num_orig_commits = commit_ids.len();

    let mut tx = workspace_command.start_transaction();
    let mut simplified_commits = 0;
    let mut edges = 0;
    let mut reparented_descendants = 0;

    tx.repo_mut()
        .transform_descendants(command.settings(), commit_ids, |mut rewriter| {
            let num_old_heads = rewriter.new_parents().len();
            if commit_ids_set.contains(rewriter.old_commit().id()) && num_old_heads > 1 {
                rewriter.simplify_ancestor_merge();
            }
            let num_new_heads = rewriter.new_parents().len();

            if rewriter.parents_changed() {
                rewriter.reparent(command.settings())?.write()?;

                if num_new_heads < num_old_heads {
                    simplified_commits += 1;
                    edges += num_old_heads - num_new_heads;
                } else {
                    reparented_descendants += 1;
                }
            }
            Ok(())
        })?;

    if let Some(mut formatter) = ui.status_formatter() {
        if simplified_commits > 0 {
            writeln!(
                formatter,
                "Removed {edges} edges from {simplified_commits} out of {num_orig_commits} \
                 commits.",
            )?;
            if reparented_descendants > 0 {
                writeln!(
                    formatter,
                    "Rebased {reparented_descendants} descendant commits",
                )?;
            }
        }
    }
    tx.finish(ui, format!("simplify {num_orig_commits} commits"))?;

    Ok(())
}
