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

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::{fmt, io};

use clap::ArgGroup;
use itertools::Itertools;
use jj_lib::git::{self, GitBranchPushTargets, GitPushError};
use jj_lib::object_id::ObjectId;
use jj_lib::op_store::RefTarget;
use jj_lib::refs::{
    classify_branch_push_action, BranchPushAction, BranchPushUpdate, LocalAndRemoteRef,
};
use jj_lib::repo::Repo;
use jj_lib::revset::{self, RevsetExpression, RevsetIteratorExt as _};
use jj_lib::settings::{ConfigResultExt as _, UserSettings};
use jj_lib::str_util::StringPattern;
use jj_lib::view::View;

use crate::cli_util::{
    short_change_hash, short_commit_hash, CommandHelper, RevisionArg, WorkspaceCommandHelper,
    WorkspaceCommandTransaction,
};
use crate::command_error::{user_error, user_error_with_hint, CommandError};
use crate::commands::git::{get_single_remote, map_git_error};
use crate::git_util::{get_git_repo, with_remote_git_callbacks, GitSidebandProgressMessageWriter};
use crate::ui::Ui;

/// Push to a Git remote
///
/// By default, pushes any branches pointing to
/// `remote_branches(remote=<remote>)..@`. Use `--branch` to push specific
/// branches. Use `--all` to push all branches. Use `--change` to generate
/// branch names based on the change IDs of specific commits.
///
/// Before the command actually moves, creates, or deletes a remote branch, it
/// makes several [safety checks]. If there is a problem, you may need to run
/// `jj git fetch --remote <remote name>` and/or resolve some [branch
/// conflicts].
///
/// [safety checks]:
///     https://martinvonz.github.io/jj/latest/branches/#pushing-branches-safety-checks
///
/// [branch conflicts]:
///     https://martinvonz.github.io/jj/latest/branches/#conflicts

#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("specific").args(&["branch", "change", "revisions"]).multiple(true)))]
#[command(group(ArgGroup::new("what").args(&["all", "deleted", "tracked"]).conflicts_with("specific")))]
pub struct PushArgs {
    /// The remote to push to (only named remotes are supported)
    #[arg(long)]
    remote: Option<String>,
    /// Push only this branch, or branches matching a pattern (can be repeated)
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select branches by wildcard pattern. For details, see
    /// https://martinvonz.github.io/jj/latest/revsets#string-patterns.
    #[arg(long, short, value_parser = StringPattern::parse)]
    branch: Vec<StringPattern>,
    /// Push all branches (including deleted branches)
    #[arg(long)]
    all: bool,
    /// Push all tracked branches (including deleted branches)
    ///
    /// This usually means that the branch was already pushed to or fetched from
    /// the relevant remote. For details, see
    /// https://martinvonz.github.io/jj/latest/branches#remotes-and-tracked-branches
    #[arg(long)]
    tracked: bool,
    /// Push all deleted branches
    ///
    /// Only tracked branches can be successfully deleted on the remote. A
    /// warning will be printed if any untracked branches on the remote
    /// correspond to missing local branches.
    #[arg(long)]
    deleted: bool,
    /// Allow pushing commits with empty descriptions
    #[arg(long)]
    allow_empty_description: bool,
    /// Push branches pointing to these commits (can be repeated)
    #[arg(long, short)]
    revisions: Vec<RevisionArg>,
    /// Push this commit by creating a branch based on its change ID (can be
    /// repeated)
    #[arg(long, short)]
    change: Vec<RevisionArg>,
    /// Only display what will change on the remote
    #[arg(long)]
    dry_run: bool,
}

fn make_branch_term(branch_names: &[impl fmt::Display]) -> String {
    match branch_names {
        [branch_name] => format!("branch {}", branch_name),
        branch_names => format!("branches {}", branch_names.iter().join(", ")),
    }
}

const DEFAULT_REMOTE: &str = "origin";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BranchMoveDirection {
    Forward,
    Backward,
    Sideways,
}

pub fn cmd_git_push(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &PushArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let git_repo = get_git_repo(workspace_command.repo().store())?;

    let remote = if let Some(name) = &args.remote {
        name.clone()
    } else {
        get_default_push_remote(ui, command.settings(), &git_repo)?
    };

    let repo = workspace_command.repo().clone();
    let mut tx = workspace_command.start_transaction();
    let tx_description;
    let mut branch_updates = vec![];
    if args.all {
        for (branch_name, targets) in repo.view().local_remote_branches(&remote) {
            match classify_branch_update(branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.to_owned(), update)),
                Ok(None) => {}
                Err(reason) => reason.print(ui)?,
            }
        }
        tx_description = format!("push all branches to git remote {remote}");
    } else if args.tracked {
        for (branch_name, targets) in repo.view().local_remote_branches(&remote) {
            if !targets.remote_ref.is_tracking() {
                continue;
            }
            match classify_branch_update(branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.to_owned(), update)),
                Ok(None) => {}
                Err(reason) => reason.print(ui)?,
            }
        }
        tx_description = format!("push all tracked branches to git remote {remote}");
    } else if args.deleted {
        for (branch_name, targets) in repo.view().local_remote_branches(&remote) {
            if targets.local_target.is_present() {
                continue;
            }
            match classify_branch_update(branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.to_owned(), update)),
                Ok(None) => {}
                Err(reason) => reason.print(ui)?,
            }
        }
        tx_description = format!("push all deleted branches to git remote {remote}");
    } else {
        let mut seen_branches: HashSet<&str> = HashSet::new();

        // Process --change branches first because matching branches can be moved.
        let change_branch_names = update_change_branches(
            ui,
            &mut tx,
            &args.change,
            &command.settings().push_branch_prefix(),
        )?;
        let change_branches = change_branch_names.iter().map(|branch_name| {
            let targets = LocalAndRemoteRef {
                local_target: tx.repo().view().get_local_branch(branch_name),
                remote_ref: tx.repo().view().get_remote_branch(branch_name, &remote),
            };
            (branch_name.as_ref(), targets)
        });
        let branches_by_name = find_branches_to_push(repo.view(), &args.branch, &remote)?;
        for (branch_name, targets) in change_branches.chain(branches_by_name.iter().copied()) {
            if !seen_branches.insert(branch_name) {
                continue;
            }
            match classify_branch_update(branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.to_owned(), update)),
                Ok(None) => writeln!(
                    ui.status(),
                    "Branch {branch_name}@{remote} already matches {branch_name}",
                )?,
                Err(reason) => return Err(reason.into()),
            }
        }

        let use_default_revset =
            args.branch.is_empty() && args.change.is_empty() && args.revisions.is_empty();
        let branches_targeted = find_branches_targeted_by_revisions(
            ui,
            tx.base_workspace_helper(),
            &remote,
            &args.revisions,
            use_default_revset,
        )?;
        for &(branch_name, targets) in &branches_targeted {
            if !seen_branches.insert(branch_name) {
                continue;
            }
            match classify_branch_update(branch_name, &remote, targets) {
                Ok(Some(update)) => branch_updates.push((branch_name.to_owned(), update)),
                Ok(None) => {}
                Err(reason) => reason.print(ui)?,
            }
        }

        tx_description = format!(
            "push {} to git remote {}",
            make_branch_term(
                &branch_updates
                    .iter()
                    .map(|(branch, _)| branch.as_str())
                    .collect_vec()
            ),
            &remote
        );
    }
    if branch_updates.is_empty() {
        writeln!(ui.status(), "Nothing changed.")?;
        return Ok(());
    }

    let mut branch_push_direction = HashMap::new();
    for (branch_name, update) in &branch_updates {
        let BranchPushUpdate {
            old_target: Some(old_target),
            new_target: Some(new_target),
        } = update
        else {
            continue;
        };
        assert_ne!(old_target, new_target);
        branch_push_direction.insert(
            branch_name.to_string(),
            if repo.index().is_ancestor(old_target, new_target) {
                BranchMoveDirection::Forward
            } else if repo.index().is_ancestor(new_target, old_target) {
                BranchMoveDirection::Backward
            } else {
                BranchMoveDirection::Sideways
            },
        );
    }

    // Check if there are conflicts in any commits we're about to push that haven't
    // already been pushed.
    let new_heads = branch_updates
        .iter()
        .filter_map(|(_, update)| update.new_target.clone())
        .collect_vec();
    let mut old_heads = repo
        .view()
        .remote_branches(&remote)
        .flat_map(|(_, old_head)| old_head.target.added_ids())
        .cloned()
        .collect_vec();
    if old_heads.is_empty() {
        old_heads.push(repo.store().root_commit_id().clone());
    }
    for commit in revset::walk_revs(repo.as_ref(), &new_heads, &old_heads)?
        .iter()
        .commits(repo.store())
    {
        let commit = commit?;
        let mut reasons = vec![];
        if commit.description().is_empty() && !args.allow_empty_description {
            reasons.push("it has no description");
        }
        if commit.author().name.is_empty()
            || commit.author().name == UserSettings::USER_NAME_PLACEHOLDER
            || commit.author().email.is_empty()
            || commit.author().email == UserSettings::USER_EMAIL_PLACEHOLDER
            || commit.committer().name.is_empty()
            || commit.committer().name == UserSettings::USER_NAME_PLACEHOLDER
            || commit.committer().email.is_empty()
            || commit.committer().email == UserSettings::USER_EMAIL_PLACEHOLDER
        {
            reasons.push("it has no author and/or committer set");
        }
        if commit.has_conflict()? {
            reasons.push("it has conflicts");
        }
        if !reasons.is_empty() {
            return Err(user_error(format!(
                "Won't push commit {} since {}",
                short_commit_hash(commit.id()),
                reasons.join(" and ")
            )));
        }
    }

    writeln!(ui.status(), "Branch changes to push to {}:", &remote)?;
    for (branch_name, update) in &branch_updates {
        match (&update.old_target, &update.new_target) {
            (Some(old_target), Some(new_target)) => {
                let old = short_commit_hash(old_target);
                let new = short_commit_hash(new_target);
                // TODO(ilyagr): Add color. Once there is color, "Move branch ... sideways" may
                // read more naturally than "Move sideways branch ...". Without color, it's hard
                // to see at a glance if one branch among many was moved sideways (say).
                // TODO: People on Discord suggest "Move branch ... forward by n commits",
                // possibly "Move branch ... sideways (X forward, Y back)".
                let msg = match branch_push_direction.get(branch_name).unwrap() {
                    BranchMoveDirection::Forward => {
                        format!("Move forward branch {branch_name} from {old} to {new}")
                    }
                    BranchMoveDirection::Backward => {
                        format!("Move backward branch {branch_name} from {old} to {new}")
                    }
                    BranchMoveDirection::Sideways => {
                        format!("Move sideways branch {branch_name} from {old} to {new}")
                    }
                };
                writeln!(ui.status(), "  {msg}")?;
            }
            (Some(old_target), None) => {
                writeln!(
                    ui.status(),
                    "  Delete branch {branch_name} from {}",
                    short_commit_hash(old_target)
                )?;
            }
            (None, Some(new_target)) => {
                writeln!(
                    ui.status(),
                    "  Add branch {branch_name} to {}",
                    short_commit_hash(new_target)
                )?;
            }
            (None, None) => {
                panic!("Not pushing any change to branch {branch_name}");
            }
        }
    }

    if args.dry_run {
        writeln!(ui.status(), "Dry-run requested, not pushing.")?;
        return Ok(());
    }

    let targets = GitBranchPushTargets { branch_updates };
    let mut writer = GitSidebandProgressMessageWriter::new(ui);
    let mut sideband_progress_callback = |progress_message: &[u8]| {
        _ = writer.write(ui, progress_message);
    };
    with_remote_git_callbacks(ui, Some(&mut sideband_progress_callback), |cb| {
        git::push_branches(tx.mut_repo(), &git_repo, &remote, &targets, cb)
    })
    .map_err(|err| match err {
        GitPushError::InternalGitError(err) => map_git_error(err),
        GitPushError::RefInUnexpectedLocation(refs) => user_error_with_hint(
            format!(
                "Refusing to push a branch that unexpectedly moved on the remote. Affected refs: \
                 {}",
                refs.join(", ")
            ),
            "Try fetching from the remote, then make the branch point to where you want it to be, \
             and push again.",
        ),
        _ => user_error(err),
    })?;
    writer.flush(ui)?;
    tx.finish(ui, tx_description)?;
    Ok(())
}

fn get_default_push_remote(
    ui: &Ui,
    settings: &UserSettings,
    git_repo: &git2::Repository,
) -> Result<String, CommandError> {
    if let Some(remote) = settings.config().get_string("git.push").optional()? {
        Ok(remote)
    } else if let Some(remote) = get_single_remote(git_repo)? {
        // similar to get_default_fetch_remotes
        if remote != DEFAULT_REMOTE {
            writeln!(
                ui.hint_default(),
                "Pushing to the only existing remote: {remote}"
            )?;
        }
        Ok(remote)
    } else {
        Ok(DEFAULT_REMOTE.to_owned())
    }
}

#[derive(Clone, Debug)]
struct RejectedBranchUpdateReason {
    message: String,
    hint: Option<String>,
}

impl RejectedBranchUpdateReason {
    fn print(&self, ui: &Ui) -> io::Result<()> {
        writeln!(ui.warning_default(), "{}", self.message)?;
        if let Some(hint) = &self.hint {
            writeln!(ui.hint_default(), "{hint}")?;
        }
        Ok(())
    }
}

impl From<RejectedBranchUpdateReason> for CommandError {
    fn from(reason: RejectedBranchUpdateReason) -> Self {
        let RejectedBranchUpdateReason { message, hint } = reason;
        let mut cmd_err = user_error(message);
        cmd_err.extend_hints(hint);
        cmd_err
    }
}

fn classify_branch_update(
    branch_name: &str,
    remote_name: &str,
    targets: LocalAndRemoteRef,
) -> Result<Option<BranchPushUpdate>, RejectedBranchUpdateReason> {
    let push_action = classify_branch_push_action(targets);
    match push_action {
        BranchPushAction::AlreadyMatches => Ok(None),
        BranchPushAction::LocalConflicted => Err(RejectedBranchUpdateReason {
            message: format!("Branch {branch_name} is conflicted"),
            hint: Some(
                "Run `jj branch list` to inspect, and use `jj branch set` to fix it up.".to_owned(),
            ),
        }),
        BranchPushAction::RemoteConflicted => Err(RejectedBranchUpdateReason {
            message: format!("Branch {branch_name}@{remote_name} is conflicted"),
            hint: Some("Run `jj git fetch` to update the conflicted remote branch.".to_owned()),
        }),
        BranchPushAction::RemoteUntracked => Err(RejectedBranchUpdateReason {
            message: format!("Non-tracking remote branch {branch_name}@{remote_name} exists"),
            hint: Some(format!(
                "Run `jj branch track {branch_name}@{remote_name}` to import the remote branch."
            )),
        }),
        BranchPushAction::Update(update) => Ok(Some(update)),
    }
}

/// Creates or moves branches based on the change IDs.
fn update_change_branches(
    ui: &Ui,
    tx: &mut WorkspaceCommandTransaction,
    changes: &[RevisionArg],
    branch_prefix: &str,
) -> Result<Vec<String>, CommandError> {
    let mut branch_names = Vec::new();
    for change_arg in changes {
        let workspace_command = tx.base_workspace_helper();
        let commit = workspace_command.resolve_single_rev(change_arg)?;
        let mut branch_name = format!("{branch_prefix}{}", commit.change_id().hex());
        let view = tx.base_repo().view();
        if view.get_local_branch(&branch_name).is_absent() {
            // A local branch with the full change ID doesn't exist already, so use the
            // short ID if it's not ambiguous (which it shouldn't be most of the time).
            let short_change_id = short_change_hash(commit.change_id());
            if workspace_command
                .resolve_single_rev(&RevisionArg::from(short_change_id.clone()))
                .is_ok()
            {
                // Short change ID is not ambiguous, so update the branch name to use it.
                branch_name = format!("{branch_prefix}{short_change_id}");
            };
        }
        if view.get_local_branch(&branch_name).is_absent() {
            writeln!(
                ui.status(),
                "Creating branch {branch_name} for revision {change_arg}",
            )?;
        }
        tx.mut_repo()
            .set_local_branch_target(&branch_name, RefTarget::normal(commit.id().clone()));
        branch_names.push(branch_name);
    }
    Ok(branch_names)
}

fn find_branches_to_push<'a>(
    view: &'a View,
    branch_patterns: &[StringPattern],
    remote_name: &str,
) -> Result<Vec<(&'a str, LocalAndRemoteRef<'a>)>, CommandError> {
    let mut matching_branches = vec![];
    let mut unmatched_patterns = vec![];
    for pattern in branch_patterns {
        let mut matches = view
            .local_remote_branches_matching(pattern, remote_name)
            .filter(|(_, targets)| {
                // If the remote exists but is not tracking, the absent local shouldn't
                // be considered a deleted branch.
                targets.local_target.is_present() || targets.remote_ref.is_tracking()
            })
            .peekable();
        if matches.peek().is_none() {
            unmatched_patterns.push(pattern);
        }
        matching_branches.extend(matches);
    }
    match &unmatched_patterns[..] {
        [] => Ok(matching_branches),
        [pattern] if pattern.is_exact() => Err(user_error(format!("No such branch: {pattern}"))),
        patterns => Err(user_error(format!(
            "No matching branches for patterns: {}",
            patterns.iter().join(", ")
        ))),
    }
}

fn find_branches_targeted_by_revisions<'a>(
    ui: &Ui,
    workspace_command: &'a WorkspaceCommandHelper,
    remote_name: &str,
    revisions: &[RevisionArg],
    use_default_revset: bool,
) -> Result<Vec<(&'a str, LocalAndRemoteRef<'a>)>, CommandError> {
    let mut revision_commit_ids = HashSet::new();
    if use_default_revset {
        let Some(wc_commit_id) = workspace_command.get_wc_commit_id().cloned() else {
            return Err(user_error("Nothing checked out in this workspace"));
        };
        let current_branches_expression = RevsetExpression::remote_branches(
            StringPattern::everything(),
            StringPattern::Exact(remote_name.to_owned()),
        )
        .range(&RevsetExpression::commit(wc_commit_id))
        .intersection(&RevsetExpression::branches(StringPattern::everything()));
        let current_branches_revset =
            current_branches_expression.evaluate_programmatic(workspace_command.repo().as_ref())?;
        revision_commit_ids.extend(current_branches_revset.iter());
        if revision_commit_ids.is_empty() {
            writeln!(
                ui.warning_default(),
                "No branches found in the default push revset: \
                 remote_branches(remote={remote_name})..@"
            )?;
        }
    }
    for rev_arg in revisions {
        let mut expression = workspace_command.parse_revset(rev_arg)?;
        expression.intersect_with(&RevsetExpression::branches(StringPattern::everything()));
        let mut commit_ids = expression.evaluate_to_commit_ids()?.peekable();
        if commit_ids.peek().is_none() {
            writeln!(
                ui.warning_default(),
                "No branches point to the specified revisions: {rev_arg}"
            )?;
        }
        revision_commit_ids.extend(commit_ids);
    }
    let branches_targeted = workspace_command
        .repo()
        .view()
        .local_remote_branches(remote_name)
        .filter(|(_, targets)| {
            let mut local_ids = targets.local_target.added_ids();
            local_ids.any(|id| revision_commit_ids.contains(id))
        })
        .collect_vec();
    Ok(branches_targeted)
}
