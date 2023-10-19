use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::io::Write as _;
use std::str::FromStr;

use clap::builder::NonEmptyStringValueParser;
use itertools::Itertools;
use jj_lib::backend::{CommitId, ObjectId};
use jj_lib::git;
use jj_lib::op_store::RefTarget;
use jj_lib::repo::Repo;
use jj_lib::revset::{self, RevsetExpression};
use jj_lib::str_util::StringPattern;
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
    Track(BranchTrackArgs),
    Untrack(BranchUntrackArgs),
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
/// By default, a tracking remote branch will be included only if its target is
/// different from the local target. A non-tracking remote branch won't be
/// listed. For a conflicted branch (both local and remote), old target
/// revisions are preceded by a "-" and new target revisions are preceded by a
/// "+".
///
/// For information about branches, see
/// https://github.com/martinvonz/jj/blob/main/docs/branches.md.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchListArgs {
    /// Show all tracking and non-tracking remote branches including the ones
    /// whose targets are synchronized with the local branches.
    #[arg(long, short, conflicts_with = "revisions")]
    all: bool,

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

/// Start tracking given remote branches
///
/// A tracking remote branch will be imported as a local branch of the same
/// name. Changes to it will propagate to the existing local branch on future
/// pulls.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchTrackArgs {
    /// Remote branches to track
    #[arg(required = true)]
    pub names: Vec<RemoteBranchName>,
}

/// Stop tracking given remote branches
///
/// A non-tracking remote branch is just a pointer to the last-fetched remote
/// branch. It won't be imported as a local branch on future pulls.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchUntrackArgs {
    /// Remote branches to untrack
    #[arg(required = true)]
    pub names: Vec<RemoteBranchName>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RemoteBranchName {
    pub branch: String,
    pub remote: String,
}

impl FromStr for RemoteBranchName {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // TODO: maybe reuse revset parser to handle branch/remote name containing @
        let (branch, remote) = s
            .rsplit_once('@')
            .ok_or("remote branch must be specified in branch@remote form")?;
        Ok(RemoteBranchName {
            branch: branch.to_owned(),
            remote: remote.to_owned(),
        })
    }
}

impl fmt::Display for RemoteBranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let RemoteBranchName { branch, remote } = self;
        write!(f, "{branch}@{remote}")
    }
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
        BranchSubcommand::Track(sub_args) => cmd_branch_track(ui, command, sub_args),
        BranchSubcommand::Untrack(sub_args) => cmd_branch_untrack(ui, command, sub_args),
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
        workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"), ui)?;
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
        workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"), ui)?;
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
            .filter_map(|(branch_name, branch_target)| {
                if glob.matches(branch_name)
                    && (allow_deleted || branch_target.local_target.is_present())
                {
                    Some(branch_name.to_owned())
                } else {
                    None
                }
            })
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
        writeln!(ui.stderr(), "Deleted {} branches.", names.len())?;
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
    if let Some(branch_name) = args.names.iter().find(|name| !view.has_branch(name)) {
        return Err(user_error(format!("No such branch: {branch_name}")));
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
        writeln!(ui.stderr(), "Forgot {} branches.", names.len())?;
    }
    Ok(())
}

fn cmd_branch_track(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchTrackArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    for name in &args.names {
        let remote_ref = view.get_remote_branch(&name.branch, &name.remote);
        if remote_ref.is_absent() {
            return Err(user_error(format!("No such remote branch: {name}")));
        }
        if remote_ref.is_tracking() {
            return Err(user_error(format!("Remote branch already tracked: {name}")));
        }
    }
    let mut tx = workspace_command
        .start_transaction(&format!("track remote {}", make_branch_term(&args.names)));
    for name in &args.names {
        tx.mut_repo()
            .track_remote_branch(&name.branch, &name.remote);
    }
    tx.finish(ui)?;
    Ok(())
}

fn cmd_branch_untrack(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchUntrackArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    for name in &args.names {
        if name.remote == git::REMOTE_NAME_FOR_LOCAL_GIT_REPO {
            // This restriction can be lifted if we want to support untracked @git branches.
            return Err(user_error("Git-tracking branch cannot be untracked"));
        }
        let remote_ref = view.get_remote_branch(&name.branch, &name.remote);
        if remote_ref.is_absent() {
            return Err(user_error(format!("No such remote branch: {name}")));
        }
        if !remote_ref.is_tracking() {
            return Err(user_error(format!("Remote branch not tracked yet: {name}")));
        }
    }
    let mut tx = workspace_command
        .start_transaction(&format!("untrack remote {}", make_branch_term(&args.names)));
    for name in &args.names {
        tx.mut_repo()
            .untrack_remote_branch(&name.branch, &name.remote);
    }
    tx.finish(ui)?;
    Ok(())
}

fn cmd_branch_list(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let view = repo.view();
    let branch_names_to_list: Option<HashSet<&str>> = if !args.revisions.is_empty() {
        // Match against local targets only, which is consistent with "jj git push".
        let filter_expressions: Vec<_> = args
            .revisions
            .iter()
            .map(|revision_str| workspace_command.parse_revset(revision_str, Some(ui)))
            .try_collect()?;
        let filter_expression = RevsetExpression::union_all(&filter_expressions);
        // Intersects with the set of local branch targets to minimize the lookup space.
        let revset_expression = RevsetExpression::branches(StringPattern::everything())
            .intersection(&filter_expression);
        let revset_expression = revset::optimize(revset_expression);
        let revset = workspace_command.evaluate_revset(revset_expression)?;
        let filtered_targets: HashSet<CommitId> = revset.iter().collect();
        // TODO: Suppose we have name-based filter like --glob, should these filters
        // be AND-ed or OR-ed? Maybe OR as "jj git push" would do. Perhaps, we
        // can consider these options as producers of branch names, not filters
        // of different kind (which are typically intersected.)
        let branch_names = view
            .local_branches()
            .filter(|(_, target)| target.added_ids().any(|id| filtered_targets.contains(id)))
            .map(|(name, _)| name)
            .collect();
        Some(branch_names)
    } else {
        None
    };

    let no_branches_template = workspace_command.parse_commit_template(
        &command
            .settings()
            .config()
            .get_string("templates.commit_summary_no_branches")?,
    )?;
    let print_branch_target =
        |formatter: &mut dyn Formatter, target: &RefTarget| -> Result<(), CommandError> {
            if let Some(id) = target.as_normal() {
                write!(formatter, ": ")?;
                let commit = repo.store().get_commit(id)?;
                no_branches_template.format(&commit, formatter)?;
                writeln!(formatter)?;
            } else {
                write!(formatter, " ")?;
                write!(formatter.labeled("conflict"), "(conflicted)")?;
                writeln!(formatter, ":")?;
                for id in target.removed_ids() {
                    let commit = repo.store().get_commit(id)?;
                    write!(formatter, "  - ")?;
                    no_branches_template.format(&commit, formatter)?;
                    writeln!(formatter)?;
                }
                for id in target.added_ids() {
                    let commit = repo.store().get_commit(id)?;
                    write!(formatter, "  + ")?;
                    no_branches_template.format(&commit, formatter)?;
                    writeln!(formatter)?;
                }
            }
            Ok(())
        };

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();

    let branches_to_list = view.branches().filter(|&(name, _)| {
        branch_names_to_list
            .as_ref()
            .map_or(true, |branch_names| branch_names.contains(name))
    });
    for (name, branch_target) in branches_to_list {
        let (tracking_remote_refs, untracked_remote_refs) =
            branch_target
                .remote_refs
                .into_iter()
                .partition::<Vec<_>, _>(|&(_, remote_ref)| remote_ref.is_tracking());

        if branch_target.local_target.is_present() || !tracking_remote_refs.is_empty() {
            write!(formatter.labeled("branch"), "{name}")?;
            if branch_target.local_target.is_present() {
                print_branch_target(formatter, branch_target.local_target)?;
            } else {
                writeln!(formatter, " (deleted)")?;
            }
        }

        for &(remote, remote_ref) in &tracking_remote_refs {
            let synced = remote_ref.target == *branch_target.local_target;
            if !args.all && synced {
                continue;
            }
            write!(formatter, "  ")?;
            write!(formatter.labeled("branch"), "@{remote}")?;
            let local_target = branch_target.local_target;
            if local_target.is_present() && !synced {
                let remote_added_ids = remote_ref.target.added_ids().cloned().collect_vec();
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
            print_branch_target(formatter, &remote_ref.target)?;
        }

        if branch_target.local_target.is_absent() && !tracking_remote_refs.is_empty() {
            let found_non_git_remote = tracking_remote_refs
                .iter()
                .any(|&(remote, _)| remote != git::REMOTE_NAME_FOR_LOCAL_GIT_REPO);
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

        if args.all {
            for &(remote, remote_ref) in &untracked_remote_refs {
                write!(formatter.labeled("branch"), "{name}@{remote}")?;
                print_branch_target(formatter, &remote_ref.target)?;
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
