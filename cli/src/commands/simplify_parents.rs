use std::collections::HashSet;

use itertools::Itertools;
use jj_lib::backend::BackendResult;
use jj_lib::revset::RevsetExpression;
use jj_lib::rewrite::CommitRewriter;
use jj_lib::settings::UserSettings;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
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
#[command(group = clap::ArgGroup::new("revision-args").multiple(true).required(true))]
pub(crate) struct SimplifyParentsArgs {
    /// Simplify specified revision(s) together with their trees of descendants
    /// (can be repeated)
    #[arg(long, short, group = "revision-args")]
    source: Vec<RevisionArg>,
    /// Simplify specified revision(s) (can be repeated)
    #[arg(long, short, group = "revision-args")]
    revisions: Vec<RevisionArg>,
}

pub(crate) fn cmd_simplify_parents(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SimplifyParentsArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let revs = RevsetExpression::descendants(
        workspace_command
            .parse_union_revsets(ui, &args.source)?
            .expression(),
    )
    .union(
        workspace_command
            .parse_union_revsets(ui, &args.revisions)?
            .expression(),
    );
    let commit_ids: Vec<_> = workspace_command
        .attach_revset_evaluator(revs)
        .evaluate_to_commit_ids()?
        .try_collect()?;
    workspace_command.check_rewritable(&commit_ids)?;
    let commit_ids_set: HashSet<_> = commit_ids.iter().cloned().collect();
    let num_orig_commits = commit_ids.len();

    let mut tx = workspace_command.start_transaction();
    let mut stats = SimplifyStats::default();
    tx.repo_mut()
        .transform_descendants(command.settings(), commit_ids, |rewriter| {
            if commit_ids_set.contains(rewriter.old_commit().id()) {
                simplify_commit_parents(command.settings(), rewriter, &mut stats)?;
            }

            Ok(())
        })?;

    if let Some(mut formatter) = ui.status_formatter() {
        if !stats.is_empty() {
            writeln!(
                formatter,
                "Removed {} edges from {} out of {} commits.",
                stats.edges, stats.commits, num_orig_commits
            )?;
        }
    }
    tx.finish(ui, format!("simplify {num_orig_commits} commits"))?;

    Ok(())
}

#[derive(Default)]
struct SimplifyStats {
    commits: usize,
    edges: usize,
}

impl SimplifyStats {
    fn is_empty(&self) -> bool {
        self.commits == 0 && self.edges == 0
    }
}

fn simplify_commit_parents(
    settings: &UserSettings,
    mut rewriter: CommitRewriter,
    stats: &mut SimplifyStats,
) -> BackendResult<()> {
    if rewriter.old_commit().parent_ids().len() <= 1 {
        return Ok(());
    }

    let num_old_heads = rewriter.new_parents().len();
    rewriter.simplify_ancestor_merge();
    let num_new_heads = rewriter.new_parents().len();

    if rewriter.parents_changed() {
        rewriter.reparent(settings)?.write()?;

        if num_new_heads < num_old_heads {
            stats.commits += 1;
            stats.edges += num_old_heads - num_new_heads;
        }
    }

    Ok(())
}
