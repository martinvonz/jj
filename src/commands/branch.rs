use std::collections::{BTreeSet, HashSet};

use clap::builder::NonEmptyStringValueParser;
use itertools::Itertools;
use jj_lib::backend::{CommitId, ObjectId};
use jj_lib::git;
use jj_lib::op_store::{BranchTarget, RefTarget, RefTargetExt as _};
use jj_lib::repo::Repo;
use jj_lib::revset::{self, RevsetExpression};
use jj_lib::view::View;

use crate::cli_util::{user_error, user_error_with_hint, CommandError, CommandHelper, RevisionArg};
use crate::commands::make_branch_term;
use crate::formatter::Formatter;
use crate::ui::Ui;

/// Manage branches.
///
/// For information about branches, see
/// https://github.com/martinvonz/jj/blob/main/docs/branches.md.
#[derive(clap::Subcommand, Clone, Debug)]
pub enum BranchSubcommand {
    #[command(visible_alias("c"))]
    Create(BranchCreateArgs),
    #[command(visible_alias("d"))]
    Delete(BranchDeleteArgs),
    #[command(visible_alias("f"))]
    Forget(BranchForgetArgs),
    #[command(visible_alias("l"))]
    List(BranchListArgs),
    #[command(visible_alias("s"))]
    Set(BranchSetArgs),
}

/// Create a new branch.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchCreateArgs {
    /// The branch's target revision.
    #[arg(long, short)]
    revision: Option<RevisionArg>,

    /// The branches to create.
    #[arg(required = true, value_parser=NonEmptyStringValueParser::new())]
    names: Vec<String>,
}

/// Delete an existing branch and propagate the deletion to remotes on the
/// next push.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchDeleteArgs {
    /// The branches to delete.
    #[arg(required_unless_present_any(& ["glob"]))]
    names: Vec<String>,

    /// A glob pattern indicating branches to delete.
    #[arg(long)]
    pub glob: Vec<String>,
}

/// List branches and their targets
///
/// A remote branch will be included only if its target is different from
/// the local target. For a conflicted branch (both local and remote), old
/// target revisions are preceded by a "-" and new target revisions are
/// preceded by a "+". For information about branches, see
/// https://github.com/martinvonz/jj/blob/main/docs/branches.md.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchListArgs {
    /// Show branches whose local targets are in the given revisions.
    ///
    /// Note that `-r deleted_branch` will not work since `deleted_branch`
    /// wouldn't have a local target.
    #[arg(long, short)]
    revisions: Vec<RevisionArg>,
}

/// Forget everything about a branch, including its local and remote
/// targets.
///
/// A forgotten branch will not impact remotes on future pushes. It will be
/// recreated on future pulls if it still exists in the remote.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchForgetArgs {
    /// The branches to forget.
    #[arg(required_unless_present_any(& ["glob"]))]
    pub names: Vec<String>,

    /// A glob pattern indicating branches to forget.
    #[arg(long)]
    pub glob: Vec<String>,
}

/// Update a given branch to point to a certain commit.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchSetArgs {
    /// The branch's target revision.
    #[arg(long, short)]
    pub revision: Option<RevisionArg>,

    /// Allow moving the branch backwards or sideways.
    #[arg(long, short = 'B')]
    pub allow_backwards: bool,

    /// The branches to update.
    #[arg(required = true)]
    pub names: Vec<String>,
}

pub fn cmd_branch(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &BranchSubcommand,
) -> Result<(), CommandError> {
    match subcommand {
        BranchSubcommand::Create(sub_args) => cmd_branch_create(ui, command, sub_args),
        BranchSubcommand::Set(sub_args) => cmd_branch_set(ui, command, sub_args),
        BranchSubcommand::Delete(sub_args) => cmd_branch_delete(ui, command, sub_args),
        BranchSubcommand::Forget(sub_args) => cmd_branch_forget(ui, command, sub_args),
        BranchSubcommand::List(sub_args) => cmd_branch_list(ui, command, sub_args),
    }
}

fn cmd_branch_create(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchCreateArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    let branch_names: Vec<&str> = args
        .names
        .iter()
        .map(|branch_name| {
            if view.get_local_branch(branch_name).is_present() {
                Err(user_error_with_hint(
                    format!("Branch already exists: {branch_name}"),
                    "Use `jj branch set` to update it.",
                ))
            } else {
                Ok(branch_name.as_str())
            }
        })
        .try_collect()?;

    if branch_names.len() > 1 {
        writeln!(
            ui.warning(),
            "warning: Creating multiple branches ({}).",
            branch_names.len()
        )?;
    }

    let target_commit =
        workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"))?;
    workspace_command.check_rewritable(&target_commit)?;
    let mut tx = workspace_command.start_transaction(&format!(
        "create {} pointing to commit {}",
        make_branch_term(&branch_names),
        target_commit.id().hex()
    ));
    for branch_name in branch_names {
        tx.mut_repo()
            .set_local_branch_target(branch_name, RefTarget::normal(target_commit.id().clone()));
    }
    tx.finish(ui)?;
    Ok(())
}

fn cmd_branch_set(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchSetArgs,
) -> Result<(), CommandError> {
    let branch_names = &args.names;
    let mut workspace_command = command.workspace_helper(ui)?;
    if branch_names.len() > 1 {
        writeln!(
            ui.warning(),
            "warning: Updating multiple branches ({}).",
            branch_names.len()
        )?;
    }

    let target_commit =
        workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"))?;
    workspace_command.check_rewritable(&target_commit)?;
    if !args.allow_backwards
        && !branch_names.iter().all(|branch_name| {
            is_fast_forward(
                workspace_command.repo().as_ref(),
                branch_name,
                target_commit.id(),
            )
        })
    {
        return Err(user_error_with_hint(
            "Refusing to move branch backwards or sideways.",
            "Use --allow-backwards to allow it.",
        ));
    }
    let mut tx = workspace_command.start_transaction(&format!(
        "point {} to commit {}",
        make_branch_term(branch_names),
        target_commit.id().hex()
    ));
    for branch_name in branch_names {
        tx.mut_repo()
            .set_local_branch_target(branch_name, RefTarget::normal(target_commit.id().clone()));
    }
    tx.finish(ui)?;
    Ok(())
}

/// This function may return the same branch more than once
fn find_globs(
    view: &View,
    globs: &[String],
    allow_deleted: bool,
) -> Result<Vec<String>, CommandError> {
    let mut matching_branches: Vec<String> = vec![];
    let mut failed_globs = vec![];
    for glob_str in globs {
        let glob = glob::Pattern::new(glob_str)?;
        let names = view
            .branches()
            .iter()
            .filter_map(|(branch_name, branch_target)| {
                if glob.matches(branch_name)
                    && (allow_deleted || branch_target.local_target.is_present())
                {
                    Some(branch_name)
                } else {
                    None
                }
            })
            .cloned()
            .collect_vec();
        if names.is_empty() {
            failed_globs.push(glob);
        }
        matching_branches.extend(names);
    }
    match &failed_globs[..] {
        [] => { /* No problem */ }
        [glob] => {
            return Err(user_error(format!(
                "The provided glob '{glob}' did not match any branches"
            )))
        }
        globs => {
            return Err(user_error(format!(
                "The provided globs '{}' did not match any branches",
                globs.iter().join("', '")
            )))
        }
    };
    Ok(matching_branches)
}

fn cmd_branch_delete(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchDeleteArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    for branch_name in &args.names {
        if workspace_command
            .repo()
            .view()
            .get_local_branch(branch_name)
            .is_absent()
        {
            return Err(user_error(format!("No such branch: {branch_name}")));
        }
    }
    let globbed_names = find_globs(view, &args.glob, false)?;
    let names: BTreeSet<String> = args.names.iter().cloned().chain(globbed_names).collect();
    let branch_term = make_branch_term(names.iter().collect_vec().as_slice());
    let mut tx = workspace_command.start_transaction(&format!("delete {branch_term}"));
    for branch_name in names.iter() {
        tx.mut_repo()
            .set_local_branch_target(branch_name, RefTarget::absent());
    }
    tx.finish(ui)?;
    if names.len() > 1 {
        writeln!(ui, "Deleted {} branches.", names.len())?;
    }
    Ok(())
}

fn cmd_branch_forget(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchForgetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    for branch_name in args.names.iter() {
        if view.get_branch(branch_name).is_none() {
            return Err(user_error(format!("No such branch: {branch_name}")));
        }
    }
    let globbed_names = find_globs(view, &args.glob, true)?;
    let names: BTreeSet<String> = args.names.iter().cloned().chain(globbed_names).collect();
    let branch_term = make_branch_term(names.iter().collect_vec().as_slice());
    let mut tx = workspace_command.start_transaction(&format!("forget {branch_term}"));
    for branch_name in names.iter() {
        tx.mut_repo().remove_branch(branch_name);
    }
    tx.finish(ui)?;
    if names.len() > 1 {
        writeln!(ui, "Forgot {} branches.", names.len())?;
    }
    Ok(())
}

fn cmd_branch_list(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let (mut all_branches, bad_branch_names) = git::build_unified_branches_map(repo.view());
    if !bad_branch_names.is_empty() {
        // TODO: This is not currently tested
        writeln!(
            ui.warning(),
            "WARNING: Branch {branch_name} has a remote-tracking branch for a remote named `git`. \
             Local-git tracking branches for it will not be shown.\nIt is recommended to rename \
             that remote, as jj normally reserves the `@git` suffix to denote local-git tracking \
             branches.",
            branch_name = bad_branch_names.join(", "),
        )?;
    }

    if !args.revisions.is_empty() {
        // Match against local targets only, which is consistent with "jj git push".
        fn local_targets(branch_target: &BranchTarget) -> impl Iterator<Item = &CommitId> {
            branch_target.local_target.added_ids()
        }

        let filter_expressions: Vec<_> = args
            .revisions
            .iter()
            .map(|revision_str| workspace_command.parse_revset(revision_str))
            .try_collect()?;
        let filter_expression = RevsetExpression::union_all(&filter_expressions);
        // Intersects with the set of all branch targets to minimize the lookup space.
        let all_targets = all_branches
            .values()
            .flat_map(local_targets)
            .cloned()
            .collect();
        let revset_expression =
            RevsetExpression::commits(all_targets).intersection(&filter_expression);
        let revset_expression = revset::optimize(revset_expression);
        let revset = workspace_command.evaluate_revset(revset_expression)?;
        let filtered_targets: HashSet<CommitId> = revset.iter().collect();
        // TODO: If we add name-based filter like --glob, this might have to be
        // rewritten as a positive list or predicate function. Should they
        // be AND-ed or OR-ed? Maybe OR as "jj git push" would do. Perhaps, we
        // can consider these options as producers of branch names, not filters
        // of different kind (which are typically intersected.)
        all_branches.retain(|_, branch_target| {
            local_targets(branch_target).any(|id| filtered_targets.contains(id))
        });
    }

    let print_branch_target =
        |formatter: &mut dyn Formatter, target: &Option<RefTarget>| -> Result<(), CommandError> {
            if let Some(id) = target.as_normal() {
                write!(formatter, ": ")?;
                let commit = repo.store().get_commit(id)?;
                workspace_command.write_commit_summary(formatter, &commit)?;
                writeln!(formatter)?;
            } else {
                write!(formatter, " ")?;
                write!(formatter.labeled("conflict"), "(conflicted)")?;
                writeln!(formatter, ":")?;
                for id in target.removed_ids() {
                    let commit = repo.store().get_commit(id)?;
                    write!(formatter, "  - ")?;
                    workspace_command.write_commit_summary(formatter, &commit)?;
                    writeln!(formatter)?;
                }
                for id in target.added_ids() {
                    let commit = repo.store().get_commit(id)?;
                    write!(formatter, "  + ")?;
                    workspace_command.write_commit_summary(formatter, &commit)?;
                    writeln!(formatter)?;
                }
            }
            Ok(())
        };

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();

    for (name, branch_target) in all_branches {
        let found_non_git_remote = {
            let pseudo_remote_count = branch_target.remote_targets.contains_key("git") as usize;
            branch_target.remote_targets.len() - pseudo_remote_count > 0
        };

        write!(formatter.labeled("branch"), "{name}")?;
        if branch_target.local_target.is_present() {
            print_branch_target(formatter, &branch_target.local_target)?;
        } else if found_non_git_remote {
            writeln!(formatter, " (deleted)")?;
        } else {
            writeln!(formatter, " (forgotten)")?;
        }

        for (remote, remote_target) in branch_target.remote_targets.iter() {
            if remote_target == &branch_target.local_target {
                continue;
            }
            write!(formatter, "  ")?;
            write!(formatter.labeled("branch"), "@{remote}")?;
            let local_target = &branch_target.local_target;
            if local_target.is_present() {
                let remote_added_ids = remote_target.added_ids().cloned().collect_vec();
                let local_added_ids = local_target.added_ids().cloned().collect_vec();
                let remote_ahead_count =
                    revset::walk_revs(repo.as_ref(), &remote_added_ids, &local_added_ids)?.count();
                let local_ahead_count =
                    revset::walk_revs(repo.as_ref(), &local_added_ids, &remote_added_ids)?.count();
                if remote_ahead_count != 0 && local_ahead_count == 0 {
                    write!(formatter, " (ahead by {remote_ahead_count} commits)")?;
                } else if remote_ahead_count == 0 && local_ahead_count != 0 {
                    write!(formatter, " (behind by {local_ahead_count} commits)")?;
                } else if remote_ahead_count != 0 && local_ahead_count != 0 {
                    write!(
                        formatter,
                        " (ahead by {remote_ahead_count} commits, behind by {local_ahead_count} \
                         commits)"
                    )?;
                }
            }
            print_branch_target(formatter, remote_target)?;
        }

        if branch_target.local_target.is_absent() {
            if found_non_git_remote {
                writeln!(
                    formatter,
                    "  (this branch will be *deleted permanently* on the remote on the\n   next \
                     `jj git push`. Use `jj branch forget` to prevent this)"
                )?;
            } else {
                writeln!(
                    formatter,
                    "  (this branch will be deleted from the underlying Git repo on the next `jj \
                     git export`)"
                )?;
            }
        }
    }

    Ok(())
}

fn is_fast_forward(repo: &dyn Repo, branch_name: &str, new_target_id: &CommitId) -> bool {
    let current_target = repo.view().get_local_branch(branch_name);
    if current_target.is_present() {
        // Strictly speaking, "all" current targets should be ancestors, but we allow
        // conflict resolution by setting branch to "any" of the old target descendants.
        current_target
            .added_ids()
            .any(|add| repo.index().is_ancestor(add, new_target_id))
    } else {
        true
    }
}
