// Copyright 2020 The Jujutsu Authors
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

#![allow(missing_docs)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::default::Default;
use std::io::Read;
use std::iter;
use std::path::PathBuf;

use git2::Oid;
use itertools::Itertools;
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::backend::{BackendError, CommitId, ObjectId};
use crate::git_backend::NO_GC_REF_NAMESPACE;
use crate::op_store::{BranchTarget, RefTarget, RefTargetOptionExt};
use crate::repo::{MutableRepo, Repo};
use crate::revset;
use crate::settings::GitSettings;
use crate::view::{RefName, View};

/// Reserved remote name for the backing Git repo.
pub const REMOTE_NAME_FOR_LOCAL_GIT_REPO: &str = "git";

#[derive(Error, Debug)]
pub enum GitImportError {
    #[error("Failed to read Git HEAD target commit {id}: {err}", id=id.hex())]
    MissingHeadTarget {
        id: CommitId,
        #[source]
        err: BackendError,
    },
    #[error("Ancestor of Git ref {ref_name} is missing: {err}")]
    MissingRefAncestor {
        ref_name: String,
        #[source]
        err: BackendError,
    },
    #[error(
        "Git remote named '{name}' is reserved for local Git repository",
        name = REMOTE_NAME_FOR_LOCAL_GIT_REPO
    )]
    RemoteReservedForLocalGitRepo,
    #[error("Unexpected git error when importing refs: {0}")]
    InternalGitError(#[from] git2::Error),
}

fn parse_git_ref(ref_name: &str) -> Option<RefName> {
    if let Some(branch_name) = ref_name.strip_prefix("refs/heads/") {
        // Git CLI says 'HEAD' is not a valid branch name
        (branch_name != "HEAD").then(|| RefName::LocalBranch(branch_name.to_string()))
    } else if let Some(remote_and_branch) = ref_name.strip_prefix("refs/remotes/") {
        remote_and_branch
            .split_once('/')
            // "refs/remotes/origin/HEAD" isn't a real remote-tracking branch
            .filter(|&(_, branch)| branch != "HEAD")
            .map(|(remote, branch)| RefName::RemoteBranch {
                remote: remote.to_string(),
                branch: branch.to_string(),
            })
    } else {
        ref_name
            .strip_prefix("refs/tags/")
            .map(|tag_name| RefName::Tag(tag_name.to_string()))
    }
}

fn to_git_ref_name(parsed_ref: &RefName) -> Option<String> {
    match parsed_ref {
        RefName::LocalBranch(branch) => {
            (!branch.is_empty() && branch != "HEAD").then(|| format!("refs/heads/{branch}"))
        }
        RefName::RemoteBranch { branch, remote } => (!branch.is_empty() && branch != "HEAD")
            .then(|| format!("refs/remotes/{remote}/{branch}")),
        RefName::Tag(tag) => Some(format!("refs/tags/{tag}")),
        RefName::GitRef(name) => Some(name.to_owned()),
    }
}

fn to_remote_branch<'a>(parsed_ref: &'a RefName, remote_name: &str) -> Option<&'a str> {
    match parsed_ref {
        RefName::RemoteBranch { branch, remote } => (remote == remote_name).then_some(branch),
        RefName::LocalBranch(..) | RefName::Tag(..) | RefName::GitRef(..) => None,
    }
}

/// Returns true if the `parsed_ref` won't be imported because its remote name
/// is reserved.
///
/// Use this as a negative `git_ref_filter` to be passed in to
/// `import_some_refs()`.
pub fn is_reserved_git_remote_ref(parsed_ref: &RefName) -> bool {
    to_remote_branch(parsed_ref, REMOTE_NAME_FOR_LOCAL_GIT_REPO).is_some()
}

/// Checks if `git_ref` points to a Git commit object, and returns its id.
///
/// If the ref points to the previously `known_target` (i.e. unchanged), this
/// should be faster than `git_ref.peel_to_commit()`.
fn resolve_git_ref_to_commit_id(
    git_ref: &git2::Reference<'_>,
    known_target: &RefTarget,
) -> Option<CommitId> {
    // Try fast path if we have a candidate id which is known to be a commit object.
    if let Some(id) = known_target.as_normal() {
        if matches!(git_ref.target(), Some(oid) if oid.as_bytes() == id.as_bytes()) {
            return Some(id.clone());
        }
        if matches!(git_ref.target_peel(), Some(oid) if oid.as_bytes() == id.as_bytes()) {
            // Perhaps an annotated tag stored in packed-refs file, and pointing to the
            // already known target commit.
            return Some(id.clone());
        }
        // A tag (according to ref name.) Try to peel one more level. This is slightly
        // faster than recurse into peel_to_commit(). If we recorded a tag oid, we
        // could skip this at all.
        if let Some(Ok(tag)) = git_ref.is_tag().then(|| git_ref.peel_to_tag()) {
            if tag.target_id().as_bytes() == id.as_bytes() {
                // An annotated tag pointing to the already known target commit.
                return Some(id.clone());
            } else {
                // Unknown id. Recurse from the current state as git_object_peel() of
                // libgit2 would do. A tag may point to non-commit object.
                let git_commit = tag.into_object().peel_to_commit().ok()?;
                return Some(CommitId::from_bytes(git_commit.id().as_bytes()));
            }
        }
    }

    let git_commit = git_ref.peel_to_commit().ok()?;
    Some(CommitId::from_bytes(git_commit.id().as_bytes()))
}

/// Builds a map of branches which also includes pseudo `@git` remote.
pub fn build_unified_branches_map(view: &View) -> BTreeMap<String, BranchTarget> {
    let mut all_branches = view.branches().clone();
    for (branch_name, git_tracking_target) in local_branch_git_tracking_refs(view) {
        // There may be a "git" remote if the view has been stored by older jj versions,
        // but we override it anyway.
        let branch_target = all_branches.entry(branch_name.to_owned()).or_default();
        branch_target.remote_targets.insert(
            REMOTE_NAME_FOR_LOCAL_GIT_REPO.to_owned(),
            git_tracking_target.clone(),
        );
    }
    all_branches
}

fn local_branch_git_tracking_refs(view: &View) -> impl Iterator<Item = (&str, &RefTarget)> {
    view.git_refs().iter().filter_map(|(ref_name, target)| {
        ref_name
            .strip_prefix("refs/heads/")
            .map(|branch_name| (branch_name, target))
    })
}

pub fn get_local_git_tracking_branch<'a>(view: &'a View, branch: &str) -> &'a RefTarget {
    view.get_git_ref(&format!("refs/heads/{branch}"))
}

fn prevent_gc(git_repo: &git2::Repository, id: &CommitId) -> Result<(), git2::Error> {
    // If multiple processes do git::import_refs() in parallel, this can fail to
    // acquire a lock file even with force=true.
    git_repo.reference(
        &format!("{}{}", NO_GC_REF_NAMESPACE, id.hex()),
        Oid::from_bytes(id.as_bytes()).unwrap(),
        true,
        "used by jj",
    )?;
    Ok(())
}

/// Reflect changes made in the underlying Git repo in the Jujutsu repo.
///
/// This function detects conflicts (if both Git and JJ modified a branch) and
/// records them in JJ's view.
pub fn import_refs(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
    git_settings: &GitSettings,
) -> Result<(), GitImportError> {
    import_some_refs(mut_repo, git_repo, git_settings, |_| true)
}

/// Reflect changes made in the underlying Git repo in the Jujutsu repo.
///
/// Only branches whose git full reference name pass the filter will be
/// considered for addition, update, or deletion.
pub fn import_some_refs(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
    git_settings: &GitSettings,
    git_ref_filter: impl Fn(&RefName) -> bool,
) -> Result<(), GitImportError> {
    // TODO: Should this be a separate function? We may not always want to import
    // the Git HEAD (and add it to our set of heads).
    let old_git_head = mut_repo.view().git_head();
    let changed_git_head = if let Ok(head_git_commit) = git_repo
        .head()
        .and_then(|head_ref| head_ref.peel_to_commit())
    {
        // The current HEAD is not added to `hidable_git_heads` because HEAD move
        // doesn't automatically mean the old HEAD branch has been rewritten.
        let head_commit_id = CommitId::from_bytes(head_git_commit.id().as_bytes());
        let new_head_target = RefTarget::normal(head_commit_id);
        (*old_git_head != new_head_target).then_some(new_head_target)
    } else {
        old_git_head.is_present().then(RefTarget::absent)
    };
    let changed_git_refs = diff_refs_to_import(mut_repo.view(), git_repo, git_ref_filter)?;
    if changed_git_refs.keys().any(is_reserved_git_remote_ref) {
        return Err(GitImportError::RemoteReservedForLocalGitRepo);
    }

    // Import new heads
    let store = mut_repo.store();
    let mut head_commits = Vec::new();
    if let Some(new_head_target) = &changed_git_head {
        for id in new_head_target.added_ids() {
            let commit = store
                .get_commit(id)
                .map_err(|err| GitImportError::MissingHeadTarget {
                    id: id.clone(),
                    err,
                })?;
            head_commits.push(commit);
        }
    }
    for (ref_name, (_, new_git_target)) in &changed_git_refs {
        for id in new_git_target.added_ids() {
            let commit =
                store
                    .get_commit(id)
                    .map_err(|err| GitImportError::MissingRefAncestor {
                        ref_name: ref_name.to_string(),
                        err,
                    })?;
            head_commits.push(commit);
        }
    }
    for commit in &head_commits {
        prevent_gc(git_repo, commit.id())?;
    }
    mut_repo.add_heads(&head_commits);

    // Apply the change that happened in git since last time we imported refs.
    if let Some(new_head_target) = changed_git_head {
        mut_repo.set_git_head_target(new_head_target);
    }
    for (ref_name, (old_git_target, new_git_target)) in &changed_git_refs {
        let full_name = to_git_ref_name(ref_name).unwrap();
        mut_repo.set_git_ref_target(&full_name, new_git_target.clone());
        if let RefName::RemoteBranch { branch, remote } = ref_name {
            // Remote-tracking branch is the last known state of the branch in the remote.
            // It shouldn't diverge even if we had inconsistent view.
            mut_repo.set_remote_branch_target(branch, remote, new_git_target.clone());
            // If a git remote-tracking branch changed, apply the change to the local branch
            // as well.
            if git_settings.auto_local_branch {
                let local_ref_name = RefName::LocalBranch(branch.clone());
                mut_repo.merge_single_ref(&local_ref_name, old_git_target, new_git_target);
            }
        } else {
            mut_repo.merge_single_ref(ref_name, old_git_target, new_git_target);
        }
    }

    // Find commits that are no longer referenced in the git repo and abandon them
    // in jj as well.
    let hidable_git_heads = changed_git_refs
        .values()
        .flat_map(|(old_git_target, _)| old_git_target.added_ids())
        .cloned()
        .collect_vec();
    if hidable_git_heads.is_empty() {
        return Ok(());
    }
    let pinned_heads = pinned_commit_ids(mut_repo.view()).cloned().collect_vec();
    // We could use mut_repo.record_rewrites() here but we know we only need to care
    // about abandoned commits for now. We may want to change this if we ever
    // add a way of preserving change IDs across rewrites by `git` (e.g. by
    // putting them in the commit message).
    let abandoned_commits = revset::walk_revs(mut_repo, &hidable_git_heads, &pinned_heads)
        .unwrap()
        .iter()
        .collect_vec();
    let root_commit_id = mut_repo.store().root_commit_id().clone();
    for abandoned_commit in abandoned_commits {
        if abandoned_commit != root_commit_id {
            mut_repo.record_abandoned_commit(abandoned_commit);
        }
    }

    Ok(())
}

/// Calculates diff of git refs to be imported.
fn diff_refs_to_import(
    view: &View,
    git_repo: &git2::Repository,
    git_ref_filter: impl Fn(&RefName) -> bool,
) -> Result<BTreeMap<RefName, (RefTarget, RefTarget)>, GitImportError> {
    let mut known_git_refs: HashMap<RefName, &RefTarget> = view
        .git_refs()
        .iter()
        .filter_map(|(full_name, target)| {
            // TODO: or clean up invalid ref in case it was stored due to historical bug?
            let ref_name = parse_git_ref(full_name).expect("stored git ref should be parsable");
            git_ref_filter(&ref_name).then_some((ref_name, target))
        })
        .collect();
    let mut changed_git_refs = BTreeMap::new();
    let git_repo_refs = git_repo.references()?;
    for git_repo_ref in git_repo_refs {
        let git_repo_ref = git_repo_ref?;
        let Some(full_name) = git_repo_ref.name() else {
            // Skip non-utf8 refs.
            continue;
        };
        let Some(ref_name) = parse_git_ref(full_name) else {
            // Skip other refs (such as notes) and symbolic refs.
            continue;
        };
        if !git_ref_filter(&ref_name) {
            continue;
        }
        let old_target = known_git_refs.get(&ref_name).copied().flatten();
        let Some(id) = resolve_git_ref_to_commit_id(&git_repo_ref, old_target) else {
            // Skip (or remove existing) invalid refs.
            continue;
        };
        // TODO: Make it configurable which remotes are publishing and update public
        // heads here.
        known_git_refs.remove(&ref_name);
        let new_target = RefTarget::normal(id);
        if new_target != *old_target {
            changed_git_refs.insert(ref_name, (old_target.clone(), new_target));
        }
    }
    for (ref_name, old_target) in known_git_refs {
        changed_git_refs.insert(ref_name, (old_target.clone(), RefTarget::absent()));
    }
    Ok(changed_git_refs)
}

/// Commits referenced by local/remote branches, tags, or HEAD@git.
///
/// On `import_refs()`, this is similar to collecting commits referenced by
/// `view.git_refs()`. Main difference is that local branches can be moved by
/// tracking remotes, and such mutation isn't applied to `view.git_refs()` yet.
fn pinned_commit_ids(view: &View) -> impl Iterator<Item = &CommitId> {
    let branch_ref_targets = view.branches().values().flat_map(|branch_target| {
        iter::once(&branch_target.local_target).chain(branch_target.remote_targets.values())
    });
    itertools::chain!(
        branch_ref_targets,
        view.tags().values(),
        iter::once(view.git_head()),
    )
    .flat_map(|target| target.added_ids())
}

#[derive(Error, Debug, PartialEq)]
pub enum GitExportError {
    #[error("Git error: {0}")]
    InternalGitError(#[from] git2::Error),
}

/// A ref we failed to export to Git, along with the reason it failed.
#[derive(Debug, PartialEq)]
pub struct FailedRefExport {
    pub name: RefName,
    pub reason: FailedRefExportReason,
}

/// The reason we failed to export a ref to Git.
#[derive(Debug, PartialEq)]
pub enum FailedRefExportReason {
    /// The name is not allowed in Git.
    InvalidGitName,
    /// The ref was in a conflicted state from the last import. A re-import
    /// should fix it.
    ConflictedOldState,
    /// The branch points to the root commit, which Git doesn't have
    OnRootCommit,
    /// We wanted to delete it, but it had been modified in Git.
    DeletedInJjModifiedInGit,
    /// We wanted to add it, but Git had added it with a different target
    AddedInJjAddedInGit,
    /// We wanted to modify it, but Git had deleted it
    ModifiedInJjDeletedInGit,
    /// Failed to delete the ref from the Git repo
    FailedToDelete(git2::Error),
    /// Failed to set the ref in the Git repo
    FailedToSet(git2::Error),
}

/// Export changes to branches made in the Jujutsu repo compared to our last
/// seen view of the Git repo in `mut_repo.view().git_refs()`. Returns a list of
/// refs that failed to export.
///
/// We ignore changed branches that are conflicted (were also changed in the Git
/// repo compared to our last remembered view of the Git repo). These will be
/// marked conflicted by the next `jj git import`.
///
/// We do not export tags and other refs at the moment, since these aren't
/// supposed to be modified by JJ. For them, the Git state is considered
/// authoritative.
// TODO: Also indicate why we failed to export these branches
pub fn export_refs(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
) -> Result<Vec<FailedRefExport>, GitExportError> {
    export_some_refs(mut_repo, git_repo, |_| true)
}

pub fn export_some_refs(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
    git_ref_filter: impl Fn(&RefName) -> bool,
) -> Result<Vec<FailedRefExport>, GitExportError> {
    // First find the changes we want need to make without modifying mut_repo
    let mut branches_to_update = BTreeMap::new();
    let mut branches_to_delete = BTreeMap::new();
    let mut failed_branches = vec![];
    let root_commit_target = RefTarget::normal(mut_repo.store().root_commit_id().clone());
    let view = mut_repo.view();
    let jj_repo_iter_all_branches = view.branches().iter().flat_map(|(branch, target)| {
        itertools::chain(
            target
                .local_target
                .is_present()
                .then(|| RefName::LocalBranch(branch.to_owned())),
            target
                .remote_targets
                .keys()
                .map(|remote| RefName::RemoteBranch {
                    branch: branch.to_string(),
                    remote: remote.to_string(),
                }),
        )
    });
    let jj_known_refs_passing_filter: HashSet<_> = view
        .git_refs()
        .keys()
        .filter_map(|name| parse_git_ref(name))
        .chain(jj_repo_iter_all_branches)
        .filter(git_ref_filter)
        .collect();
    for jj_known_ref in jj_known_refs_passing_filter {
        let new_branch = match &jj_known_ref {
            RefName::LocalBranch(branch) => view.get_local_branch(branch),
            RefName::RemoteBranch { remote, branch } => {
                // Currently, the only situation where this case occurs *and* new_branch !=
                // old_branch is after a `jj branch forget`. So, in practice, for
                // remote-tracking branches either `new_branch == old_branch` or
                // `new_branch == None`.
                view.get_remote_branch(branch, remote)
            }
            _ => continue,
        };
        let old_branch = if let Some(name) = to_git_ref_name(&jj_known_ref) {
            view.get_git_ref(&name)
        } else {
            // Invalid branch name in Git sense
            failed_branches.push(FailedRefExport {
                name: jj_known_ref,
                reason: FailedRefExportReason::InvalidGitName,
            });
            continue;
        };
        if new_branch == old_branch {
            continue;
        }
        if *new_branch == root_commit_target {
            // Git doesn't have a root commit
            failed_branches.push(FailedRefExport {
                name: jj_known_ref,
                reason: FailedRefExportReason::OnRootCommit,
            });
            continue;
        }
        let old_oid = if let Some(id) = old_branch.as_normal() {
            Some(Oid::from_bytes(id.as_bytes()).unwrap())
        } else if old_branch.has_conflict() {
            // The old git ref should only be a conflict if there were concurrent import
            // operations while the value changed. Don't overwrite these values.
            failed_branches.push(FailedRefExport {
                name: jj_known_ref,
                reason: FailedRefExportReason::ConflictedOldState,
            });
            continue;
        } else {
            assert!(old_branch.is_absent());
            None
        };
        if let Some(id) = new_branch.as_normal() {
            let new_oid = Oid::from_bytes(id.as_bytes());
            branches_to_update.insert(jj_known_ref, (old_oid, new_oid.unwrap()));
        } else if new_branch.has_conflict() {
            // Skip conflicts and leave the old value in git_refs
            continue;
        } else {
            assert!(new_branch.is_absent());
            branches_to_delete.insert(jj_known_ref, old_oid.unwrap());
        }
    }
    // TODO: Also check other worktrees' HEAD.
    if let Ok(head_ref) = git_repo.find_reference("HEAD") {
        if let (Some(head_git_ref), Ok(current_git_commit)) =
            (head_ref.symbolic_target(), head_ref.peel_to_commit())
        {
            if let Some(parsed_ref) = parse_git_ref(head_git_ref) {
                let detach_head =
                    if let Some((_old_oid, new_oid)) = branches_to_update.get(&parsed_ref) {
                        *new_oid != current_git_commit.id()
                    } else {
                        branches_to_delete.contains_key(&parsed_ref)
                    };
                if detach_head {
                    git_repo.set_head_detached(current_git_commit.id())?;
                }
            }
        }
    }
    for (parsed_ref_name, old_oid) in branches_to_delete {
        let git_ref_name = to_git_ref_name(&parsed_ref_name).unwrap();
        let reason = if let Ok(mut git_repo_ref) = git_repo.find_reference(&git_ref_name) {
            if git_repo_ref.target() == Some(old_oid) {
                // The branch has not been updated by git, so go ahead and delete it
                git_repo_ref
                    .delete()
                    .err()
                    .map(FailedRefExportReason::FailedToDelete)
            } else {
                // The branch was updated by git
                Some(FailedRefExportReason::DeletedInJjModifiedInGit)
            }
        } else {
            // The branch is already deleted
            None
        };
        if let Some(reason) = reason {
            failed_branches.push(FailedRefExport {
                name: parsed_ref_name,
                reason,
            });
        } else {
            mut_repo.set_git_ref_target(&git_ref_name, RefTarget::absent());
        }
    }
    for (parsed_ref_name, (old_oid, new_oid)) in branches_to_update {
        let git_ref_name = to_git_ref_name(&parsed_ref_name).unwrap();
        let reason = match old_oid {
            None => {
                if let Ok(git_repo_ref) = git_repo.find_reference(&git_ref_name) {
                    // The branch was added in jj and in git. We're good if and only if git
                    // pointed it to our desired target.
                    if git_repo_ref.target() == Some(new_oid) {
                        None
                    } else {
                        Some(FailedRefExportReason::AddedInJjAddedInGit)
                    }
                } else {
                    // The branch was added in jj but still doesn't exist in git, so add it
                    git_repo
                        .reference(&git_ref_name, new_oid, true, "export from jj")
                        .err()
                        .map(FailedRefExportReason::FailedToSet)
                }
            }
            Some(old_oid) => {
                // The branch was modified in jj. We can use libgit2's API for updating under a
                // lock.
                if let Err(err) = git_repo.reference_matching(
                    &git_ref_name,
                    new_oid,
                    true,
                    old_oid,
                    "export from jj",
                ) {
                    // The reference was probably updated in git
                    if let Ok(git_repo_ref) = git_repo.find_reference(&git_ref_name) {
                        // We still consider this a success if it was updated to our desired target
                        if git_repo_ref.target() == Some(new_oid) {
                            None
                        } else {
                            Some(FailedRefExportReason::FailedToSet(err))
                        }
                    } else {
                        // The reference was deleted in git and moved in jj
                        Some(FailedRefExportReason::ModifiedInJjDeletedInGit)
                    }
                } else {
                    // Successfully updated from old_oid to new_oid (unchanged in git)
                    None
                }
            }
        };
        if let Some(reason) = reason {
            failed_branches.push(FailedRefExport {
                name: parsed_ref_name,
                reason,
            });
        } else {
            mut_repo.set_git_ref_target(
                &git_ref_name,
                RefTarget::normal(CommitId::from_bytes(new_oid.as_bytes())),
            );
        }
    }
    failed_branches.sort_by_key(|failed| failed.name.clone());
    Ok(failed_branches)
}

#[derive(Debug, Error)]
pub enum GitRemoteManagementError {
    #[error("No git remote named '{0}'")]
    NoSuchRemote(String),
    #[error("Git remote named '{0}' already exists")]
    RemoteAlreadyExists(String),
    #[error(
        "Git remote named '{name}' is reserved for local Git repository",
        name = REMOTE_NAME_FOR_LOCAL_GIT_REPO
    )]
    RemoteReservedForLocalGitRepo,
    #[error(transparent)]
    InternalGitError(git2::Error),
}

fn is_remote_not_found_err(err: &git2::Error) -> bool {
    matches!(
        (err.class(), err.code()),
        (
            git2::ErrorClass::Config,
            git2::ErrorCode::NotFound | git2::ErrorCode::InvalidSpec
        )
    )
}

fn is_remote_exists_err(err: &git2::Error) -> bool {
    matches!(
        (err.class(), err.code()),
        (git2::ErrorClass::Config, git2::ErrorCode::Exists)
    )
}

pub fn add_remote(
    git_repo: &git2::Repository,
    remote_name: &str,
    url: &str,
) -> Result<(), GitRemoteManagementError> {
    if remote_name == REMOTE_NAME_FOR_LOCAL_GIT_REPO {
        return Err(GitRemoteManagementError::RemoteReservedForLocalGitRepo);
    }
    git_repo.remote(remote_name, url).map_err(|err| {
        if is_remote_exists_err(&err) {
            GitRemoteManagementError::RemoteAlreadyExists(remote_name.to_owned())
        } else {
            GitRemoteManagementError::InternalGitError(err)
        }
    })?;
    Ok(())
}

pub fn remove_remote(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
    remote_name: &str,
) -> Result<(), GitRemoteManagementError> {
    git_repo.remote_delete(remote_name).map_err(|err| {
        if is_remote_not_found_err(&err) {
            GitRemoteManagementError::NoSuchRemote(remote_name.to_owned())
        } else {
            GitRemoteManagementError::InternalGitError(err)
        }
    })?;
    let mut branches_to_delete = vec![];
    for (branch, target) in mut_repo.view().branches() {
        if target.remote_targets.contains_key(remote_name) {
            branches_to_delete.push(branch.clone());
        }
    }
    let prefix = format!("refs/remotes/{remote_name}/");
    let git_refs_to_delete = mut_repo
        .view()
        .git_refs()
        .keys()
        .filter(|&r| r.starts_with(&prefix))
        .cloned()
        .collect_vec();
    for branch in branches_to_delete {
        mut_repo.set_remote_branch_target(&branch, remote_name, RefTarget::absent());
    }
    for git_ref in git_refs_to_delete {
        mut_repo.set_git_ref_target(&git_ref, RefTarget::absent());
    }
    Ok(())
}

pub fn rename_remote(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
    old_remote_name: &str,
    new_remote_name: &str,
) -> Result<(), GitRemoteManagementError> {
    if new_remote_name == REMOTE_NAME_FOR_LOCAL_GIT_REPO {
        return Err(GitRemoteManagementError::RemoteReservedForLocalGitRepo);
    }
    git_repo
        .remote_rename(old_remote_name, new_remote_name)
        .map_err(|err| {
            if is_remote_not_found_err(&err) {
                GitRemoteManagementError::NoSuchRemote(old_remote_name.to_owned())
            } else if is_remote_exists_err(&err) {
                GitRemoteManagementError::RemoteAlreadyExists(new_remote_name.to_owned())
            } else {
                GitRemoteManagementError::InternalGitError(err)
            }
        })?;
    mut_repo.rename_remote(old_remote_name, new_remote_name);
    let prefix = format!("refs/remotes/{old_remote_name}/");
    let git_refs = mut_repo
        .view()
        .git_refs()
        .iter()
        .filter_map(|(r, target)| {
            r.strip_prefix(&prefix).map(|p| {
                (
                    r.clone(),
                    format!("refs/remotes/{new_remote_name}/{p}"),
                    target.clone(),
                )
            })
        })
        .collect_vec();
    for (old, new, target) in git_refs {
        mut_repo.set_git_ref_target(&old, RefTarget::absent());
        mut_repo.set_git_ref_target(&new, target);
    }
    Ok(())
}

#[derive(Error, Debug)]
pub enum GitFetchError {
    #[error("No git remote named '{0}'")]
    NoSuchRemote(String),
    #[error("Invalid glob provided. Globs may not contain the characters `:` or `^`.")]
    InvalidGlob,
    #[error("Failed to import Git refs: {0}")]
    GitImportError(#[from] GitImportError),
    // TODO: I'm sure there are other errors possible, such as transport-level errors.
    #[error("Unexpected git error when fetching: {0}")]
    InternalGitError(#[from] git2::Error),
}

#[tracing::instrument(skip(mut_repo, git_repo, callbacks))]
pub fn fetch(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
    remote_name: &str,
    branch_name_globs: Option<&[&str]>,
    callbacks: RemoteCallbacks<'_>,
    git_settings: &GitSettings,
) -> Result<Option<String>, GitFetchError> {
    let branch_name_filter = {
        let regex = if let Some(globs) = branch_name_globs {
            let result = regex::RegexSet::new(
                globs
                    .iter()
                    .map(|glob| format!("^{}$", glob.replace('*', ".*"))),
            )
            .map_err(|_| GitFetchError::InvalidGlob)?;
            tracing::debug!(?globs, ?result, "globs as regex");
            Some(result)
        } else {
            None
        };
        move |branch: &str| regex.as_ref().map(|r| r.is_match(branch)).unwrap_or(true)
    };

    // In non-colocated repositories, it's possible that `jj branch forget` was run
    // at some point and no `jj git export` happened since.
    //
    // This would mean that remote-tracking branches, forgotten in the jj repo,
    // still exist in the git repo. If the branches didn't move on the remote, and
    // we fetched them, jj would think that they are unmodified and wouldn't
    // resurrect them.
    //
    // Export will delete the remote-tracking branches in the git repo, so it's
    // possible to fetch them again.
    //
    // For more details, see the `test_branch_forget_fetched_branch` test, and PRs
    // #1714 and #1771
    //
    // Apart from `jj branch forget`, jj doesn't provide commands to manipulate
    // remote-tracking branches, and local git branches don't affect fetch
    // behaviors. So, it's unnecessary to export anything else.
    //
    // TODO: Create a command the user can use to reset jj's
    // branch state to the git repo's state. In this case, `jj branch forget`
    // doesn't work as it tries to delete the latter. One possible name is `jj
    // git import --reset BRANCH`.
    // TODO: Once the command described above exists, it should be mentioned in `jj
    // help branch forget`.
    let nonempty_branches: HashSet<_> = mut_repo
        .view()
        .branches()
        .iter()
        .filter(|&(_branch, target)| target.local_target.is_present())
        .map(|(branch, _target)| branch.to_owned())
        .collect();
    // TODO: Inform the user if the export failed? In most cases, export is not
    // essential for fetch to work.
    let _ = export_some_refs(mut_repo, git_repo, |ref_name| {
        to_remote_branch(ref_name, remote_name)
            .map(|branch| branch_name_filter(branch) && !nonempty_branches.contains(branch))
            .unwrap_or(false)
    });

    // Perform a `git fetch` on the local git repo, updating the remote-tracking
    // branches in the git repo.
    let mut remote = git_repo.find_remote(remote_name).map_err(|err| {
        if is_remote_not_found_err(&err) {
            GitFetchError::NoSuchRemote(remote_name.to_string())
        } else {
            GitFetchError::InternalGitError(err)
        }
    })?;
    let mut fetch_options = git2::FetchOptions::new();
    let mut proxy_options = git2::ProxyOptions::new();
    proxy_options.auto();
    fetch_options.proxy_options(proxy_options);
    let callbacks = callbacks.into_git();
    fetch_options.remote_callbacks(callbacks);
    let refspecs = {
        // If no globs have been given, import all branches
        let globs = branch_name_globs.unwrap_or(&["*"]);
        if globs.iter().any(|g| g.contains(|c| ":^".contains(c))) {
            return Err(GitFetchError::InvalidGlob);
        }
        // At this point, we are only updating Git's remote tracking branches, not the
        // local branches.
        globs
            .iter()
            .map(|glob| format!("+refs/heads/{glob}:refs/remotes/{remote_name}/{glob}"))
            .collect_vec()
    };
    tracing::debug!("remote.download");
    remote.download(&refspecs, Some(&mut fetch_options))?;
    tracing::debug!("remote.prune");
    remote.prune(None)?;
    tracing::debug!("remote.update_tips");
    remote.update_tips(None, false, git2::AutotagOption::Unspecified, None)?;
    // TODO: We could make it optional to get the default branch since we only care
    // about it on clone.
    let mut default_branch = None;
    if let Ok(default_ref_buf) = remote.default_branch() {
        if let Some(default_ref) = default_ref_buf.as_str() {
            // LocalBranch here is the local branch on the remote, so it's really the remote
            // branch
            if let Some(RefName::LocalBranch(branch_name)) = parse_git_ref(default_ref) {
                tracing::debug!(default_branch = branch_name);
                default_branch = Some(branch_name);
            }
        }
    }
    tracing::debug!("remote.disconnect");
    remote.disconnect()?;

    // `import_some_refs` will import the remote-tracking branches into the jj repo
    // and update jj's local branches.
    tracing::debug!("import_refs");
    import_some_refs(mut_repo, git_repo, git_settings, |ref_name| {
        to_remote_branch(ref_name, remote_name)
            .map(&branch_name_filter)
            .unwrap_or(false)
    })?;
    Ok(default_branch)
}

#[derive(Error, Debug, PartialEq)]
pub enum GitPushError {
    #[error("No git remote named '{0}'")]
    NoSuchRemote(String),
    #[error("Push is not fast-forwardable")]
    NotFastForward,
    #[error("Remote rejected the update of some refs (do you have permission to push to {0:?}?)")]
    RefUpdateRejected(Vec<String>),
    // TODO: I'm sure there are other errors possible, such as transport-level errors,
    // and errors caused by the remote rejecting the push.
    #[error("Unexpected git error when pushing: {0}")]
    InternalGitError(#[from] git2::Error),
}

pub struct GitRefUpdate {
    pub qualified_name: String,
    // TODO: We want this to be a `current_target: Option<CommitId>` for the expected current
    // commit on the remote. It's a blunt "force" option instead until git2-rs supports the
    // "push negotiation" callback (https://github.com/rust-lang/git2-rs/issues/733).
    pub force: bool,
    pub new_target: Option<CommitId>,
}

pub fn push_updates(
    git_repo: &git2::Repository,
    remote_name: &str,
    updates: &[GitRefUpdate],
    callbacks: RemoteCallbacks<'_>,
) -> Result<(), GitPushError> {
    let mut temp_refs = vec![];
    let mut qualified_remote_refs = vec![];
    let mut refspecs = vec![];
    for update in updates {
        qualified_remote_refs.push(update.qualified_name.as_str());
        if let Some(new_target) = &update.new_target {
            // Create a temporary ref to work around https://github.com/libgit2/libgit2/issues/3178
            let temp_ref_name = format!("refs/jj/git-push/{}", new_target.hex());
            temp_refs.push(git_repo.reference(
                &temp_ref_name,
                git2::Oid::from_bytes(new_target.as_bytes()).unwrap(),
                true,
                "temporary reference for git push",
            )?);
            refspecs.push(format!(
                "{}{}:{}",
                (if update.force { "+" } else { "" }),
                temp_ref_name,
                update.qualified_name
            ));
        } else {
            refspecs.push(format!(":{}", update.qualified_name));
        }
    }
    let result = push_refs(
        git_repo,
        remote_name,
        &qualified_remote_refs,
        &refspecs,
        callbacks,
    );
    for mut temp_ref in temp_refs {
        // TODO: Figure out how to do the equivalent of absl::Cleanup for
        // temp_ref.delete().
        if let Err(err) = temp_ref.delete() {
            // Propagate error only if we don't already have an error to return and it's not
            // NotFound (there may be duplicates if the list if multiple branches moved to
            // the same commit).
            if result.is_ok() && err.code() != git2::ErrorCode::NotFound {
                return Err(GitPushError::InternalGitError(err));
            }
        }
    }
    result
}

fn push_refs(
    git_repo: &git2::Repository,
    remote_name: &str,
    qualified_remote_refs: &[&str],
    refspecs: &[String],
    callbacks: RemoteCallbacks<'_>,
) -> Result<(), GitPushError> {
    let mut remote = git_repo.find_remote(remote_name).map_err(|err| {
        if is_remote_not_found_err(&err) {
            GitPushError::NoSuchRemote(remote_name.to_string())
        } else {
            GitPushError::InternalGitError(err)
        }
    })?;
    let mut remaining_remote_refs: HashSet<_> = qualified_remote_refs.iter().copied().collect();
    let mut push_options = git2::PushOptions::new();
    let mut proxy_options = git2::ProxyOptions::new();
    proxy_options.auto();
    push_options.proxy_options(proxy_options);
    let mut callbacks = callbacks.into_git();
    callbacks.push_update_reference(|refname, status| {
        // The status is Some if the ref update was rejected
        if status.is_none() {
            remaining_remote_refs.remove(refname);
        }
        Ok(())
    });
    push_options.remote_callbacks(callbacks);
    remote
        .push(refspecs, Some(&mut push_options))
        .map_err(|err| match (err.class(), err.code()) {
            (git2::ErrorClass::Reference, git2::ErrorCode::NotFastForward) => {
                GitPushError::NotFastForward
            }
            _ => GitPushError::InternalGitError(err),
        })?;
    drop(push_options);
    if remaining_remote_refs.is_empty() {
        Ok(())
    } else {
        Err(GitPushError::RefUpdateRejected(
            remaining_remote_refs
                .iter()
                .sorted()
                .map(|name| name.to_string())
                .collect(),
        ))
    }
}

#[non_exhaustive]
#[derive(Default)]
#[allow(clippy::type_complexity)]
pub struct RemoteCallbacks<'a> {
    pub progress: Option<&'a mut dyn FnMut(&Progress)>,
    pub get_ssh_keys: Option<&'a mut dyn FnMut(&str) -> Vec<PathBuf>>,
    pub get_password: Option<&'a mut dyn FnMut(&str, &str) -> Option<String>>,
    pub get_username_password: Option<&'a mut dyn FnMut(&str) -> Option<(String, String)>>,
}

impl<'a> RemoteCallbacks<'a> {
    fn into_git(mut self) -> git2::RemoteCallbacks<'a> {
        let mut callbacks = git2::RemoteCallbacks::new();
        if let Some(progress_cb) = self.progress {
            callbacks.transfer_progress(move |progress| {
                progress_cb(&Progress {
                    bytes_downloaded: (progress.received_objects() < progress.total_objects())
                        .then(|| progress.received_bytes() as u64),
                    overall: (progress.indexed_objects() + progress.indexed_deltas()) as f32
                        / (progress.total_objects() + progress.total_deltas()) as f32,
                });
                true
            });
        }
        // TODO: We should expose the callbacks to the caller instead -- the library
        // crate shouldn't read environment variables.
        let mut tried_ssh_agent = false;
        let mut ssh_key_paths_to_try: Option<Vec<PathBuf>> = None;
        callbacks.credentials(move |url, username_from_url, allowed_types| {
            let span = tracing::debug_span!("RemoteCallbacks.credentials");
            let _ = span.enter();

            let git_config = git2::Config::open_default();
            let credential_helper = git_config
                .and_then(|conf| git2::Cred::credential_helper(&conf, url, username_from_url));
            if let Ok(creds) = credential_helper {
                tracing::info!("using credential_helper");
                return Ok(creds);
            } else if let Some(username) = username_from_url {
                if allowed_types.contains(git2::CredentialType::SSH_KEY) {
                    // Try to get the SSH key from the agent once. We don't even check if
                    // $SSH_AUTH_SOCK is set because Windows uses another mechanism.
                    if !tried_ssh_agent {
                        tracing::info!(username, "trying ssh_key_from_agent");
                        tried_ssh_agent = true;
                        return git2::Cred::ssh_key_from_agent(username).map_err(|err| {
                            tracing::error!(err = %err);
                            err
                        });
                    }

                    let paths = ssh_key_paths_to_try.get_or_insert_with(|| {
                        if let Some(ref mut cb) = self.get_ssh_keys {
                            let mut paths = cb(username);
                            paths.reverse();
                            paths
                        } else {
                            vec![]
                        }
                    });

                    if let Some(path) = paths.pop() {
                        tracing::info!(username, path = ?path, "trying ssh_key");
                        return git2::Cred::ssh_key(username, None, &path, None).map_err(|err| {
                            tracing::error!(err = %err);
                            err
                        });
                    }
                }
                if allowed_types.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
                    if let Some(ref mut cb) = self.get_password {
                        if let Some(pw) = cb(url, username) {
                            tracing::info!(
                                username,
                                "using userpass_plaintext with username from url"
                            );
                            return git2::Cred::userpass_plaintext(username, &pw).map_err(|err| {
                                tracing::error!(err = %err);
                                err
                            });
                        }
                    }
                }
            } else if allowed_types.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
                if let Some(ref mut cb) = self.get_username_password {
                    if let Some((username, pw)) = cb(url) {
                        tracing::info!(username, "using userpass_plaintext");
                        return git2::Cred::userpass_plaintext(&username, &pw).map_err(|err| {
                            tracing::error!(err = %err);
                            err
                        });
                    }
                }
            }
            tracing::info!("using default");
            git2::Cred::default()
        });
        callbacks
    }
}

pub struct Progress {
    /// `Some` iff data transfer is currently in progress
    pub bytes_downloaded: Option<u64>,
    pub overall: f32,
}

#[derive(Default)]
struct PartialSubmoduleConfig {
    path: Option<String>,
    url: Option<String>,
}

/// Represents configuration from a submodule, e.g. in .gitmodules
/// This doesn't include all possible fields, only the ones we care about
#[derive(Debug, PartialEq, Eq)]
pub struct SubmoduleConfig {
    pub name: String,
    pub path: String,
    pub url: String,
}

#[derive(Error, Debug)]
pub enum GitConfigParseError {
    #[error("Unexpected io error when parsing config: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Unexpected git error when parsing config: {0}")]
    InternalGitError(#[from] git2::Error),
}

pub fn parse_gitmodules(
    config: &mut dyn Read,
) -> Result<BTreeMap<String, SubmoduleConfig>, GitConfigParseError> {
    // git2 can only read from a path, so set one up
    let mut temp_file = NamedTempFile::new()?;
    std::io::copy(config, &mut temp_file)?;
    let path = temp_file.into_temp_path();
    let git_config = git2::Config::open(&path)?;
    // Partial config value for each submodule name
    let mut partial_configs: BTreeMap<String, PartialSubmoduleConfig> = BTreeMap::new();

    let entries = git_config.entries(Some(r"submodule\..+\."))?;
    entries.for_each(|entry| {
        let (config_name, config_value) = match (entry.name(), entry.value()) {
            // Reject non-utf8 entries
            (Some(name), Some(value)) => (name, value),
            _ => return,
        };

        // config_name is of the form submodule.<name>.<variable>
        let (submod_name, submod_var) = config_name
            .strip_prefix("submodule.")
            .unwrap()
            .split_once('.')
            .unwrap();

        let map_entry = partial_configs.entry(submod_name.to_string()).or_default();

        match (submod_var.to_ascii_lowercase().as_str(), &map_entry) {
            // TODO Git warns when a duplicate config entry is found, we should
            // consider doing the same.
            ("path", PartialSubmoduleConfig { path: None, .. }) => {
                map_entry.path = Some(config_value.to_string())
            }
            ("url", PartialSubmoduleConfig { url: None, .. }) => {
                map_entry.url = Some(config_value.to_string())
            }
            _ => (),
        };
    })?;

    let ret = partial_configs
        .into_iter()
        .filter_map(|(name, val)| {
            Some((
                name.clone(),
                SubmoduleConfig {
                    name,
                    path: val.path?,
                    url: val.url?,
                },
            ))
        })
        .collect();
    Ok(ret)
}
