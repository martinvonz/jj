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

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::default::Default;
use std::io::Read;
use std::path::PathBuf;
use std::{fmt, str};

use git2::Oid;
use itertools::Itertools;
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::backend::{BackendError, CommitId};
use crate::commit::Commit;
use crate::git_backend::GitBackend;
use crate::index::Index;
use crate::object_id::ObjectId;
use crate::op_store::{RefTarget, RefTargetOptionExt, RemoteRef, RemoteRefState};
use crate::refs::{self, BranchPushUpdate};
use crate::repo::{MutableRepo, Repo};
use crate::revset::RevsetExpression;
use crate::settings::GitSettings;
use crate::store::Store;
use crate::str_util::StringPattern;
use crate::view::View;

/// Reserved remote name for the backing Git repo.
pub const REMOTE_NAME_FOR_LOCAL_GIT_REPO: &str = "git";
/// Ref name used as a placeholder to unset HEAD without a commit.
const UNBORN_ROOT_REF_NAME: &str = "refs/jj/root";

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash, Debug)]
pub enum RefName {
    LocalBranch(String),
    RemoteBranch { branch: String, remote: String },
    Tag(String),
}

impl fmt::Display for RefName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RefName::LocalBranch(name) => write!(f, "{name}"),
            RefName::RemoteBranch { branch, remote } => write!(f, "{branch}@{remote}"),
            RefName::Tag(name) => write!(f, "{name}"),
        }
    }
}

pub fn parse_git_ref(ref_name: &str) -> Option<RefName> {
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
    }
}

fn to_remote_branch<'a>(parsed_ref: &'a RefName, remote_name: &str) -> Option<&'a str> {
    match parsed_ref {
        RefName::RemoteBranch { branch, remote } => (remote == remote_name).then_some(branch),
        RefName::LocalBranch(..) | RefName::Tag(..) => None,
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

fn get_git_backend(store: &Store) -> Option<&GitBackend> {
    store.backend_impl().downcast_ref()
}

fn get_git_repo(store: &Store) -> Option<gix::Repository> {
    get_git_backend(store).map(|backend| backend.git_repo())
}

/// Checks if `git_ref` points to a Git commit object, and returns its id.
///
/// If the ref points to the previously `known_target` (i.e. unchanged), this
/// should be faster than `git_ref.into_fully_peeled_id()`.
fn resolve_git_ref_to_commit_id(
    git_ref: &gix::Reference,
    known_target: &RefTarget,
) -> Option<CommitId> {
    let mut peeling_ref = Cow::Borrowed(git_ref);

    // Try fast path if we have a candidate id which is known to be a commit object.
    if let Some(id) = known_target.as_normal() {
        let raw_ref = &git_ref.inner;
        if matches!(raw_ref.target.try_id(), Some(oid) if oid.as_bytes() == id.as_bytes()) {
            return Some(id.clone());
        }
        if matches!(raw_ref.peeled, Some(oid) if oid.as_bytes() == id.as_bytes()) {
            // Perhaps an annotated tag stored in packed-refs file, and pointing to the
            // already known target commit.
            return Some(id.clone());
        }
        // A tag (according to ref name.) Try to peel one more level. This is slightly
        // faster than recurse into into_fully_peeled_id(). If we recorded a tag oid, we
        // could skip this at all.
        if raw_ref.peeled.is_none() && git_ref.name().as_bstr().starts_with(b"refs/tags/") {
            let maybe_tag = git_ref
                .try_id()
                .and_then(|id| id.object().ok())
                .and_then(|object| object.try_into_tag().ok());
            if let Some(oid) = maybe_tag.as_ref().and_then(|tag| tag.target_id().ok()) {
                if oid.as_bytes() == id.as_bytes() {
                    // An annotated tag pointing to the already known target commit.
                    return Some(id.clone());
                }
                // Unknown id. Recurse from the current state. A tag may point to
                // non-commit object.
                peeling_ref.to_mut().inner.target = gix::refs::Target::Peeled(oid.detach());
            }
        }
    }

    // Alternatively, we might want to inline the first half of the peeling
    // loop. into_fully_peeled_id() looks up the target object to see if it's
    // a tag or not, and we need to check if it's a commit object.
    let peeled_id = peeling_ref.into_owned().into_fully_peeled_id().ok()?;
    let is_commit = peeled_id
        .object()
        .map_or(false, |object| object.kind.is_commit());
    is_commit.then(|| CommitId::from_bytes(peeled_id.as_bytes()))
}

#[derive(Error, Debug)]
pub enum GitImportError {
    #[error("Failed to read Git HEAD target commit {id}", id=id.hex())]
    MissingHeadTarget {
        id: CommitId,
        #[source]
        err: BackendError,
    },
    #[error("Ancestor of Git ref {ref_name} is missing")]
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
    #[error("Unexpected backend error when importing refs")]
    InternalBackend(#[source] BackendError),
    #[error("Unexpected git error when importing refs")]
    InternalGitError(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("The repo is not backed by a Git repo")]
    UnexpectedBackend,
}

impl GitImportError {
    fn from_git(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        GitImportError::InternalGitError(source.into())
    }
}

/// Describes changes made by `import_refs()` or `fetch()`.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct GitImportStats {
    /// Commits superseded by newly imported commits.
    pub abandoned_commits: Vec<CommitId>,
    /// Remote `(ref_name, (old_remote_ref, new_target))`s to be merged in to
    /// the local refs.
    pub changed_remote_refs: BTreeMap<RefName, (RemoteRef, RefTarget)>,
}

#[derive(Debug)]
struct RefsToImport {
    /// Git ref `(full_name, new_target)`s to be copied to the view.
    changed_git_refs: Vec<(String, RefTarget)>,
    /// Remote `(ref_name, (old_remote_ref, new_target))`s to be merged in to
    /// the local refs.
    changed_remote_refs: BTreeMap<RefName, (RemoteRef, RefTarget)>,
}

/// Reflect changes made in the underlying Git repo in the Jujutsu repo.
///
/// This function detects conflicts (if both Git and JJ modified a branch) and
/// records them in JJ's view.
pub fn import_refs(
    mut_repo: &mut MutableRepo,
    git_settings: &GitSettings,
) -> Result<GitImportStats, GitImportError> {
    import_some_refs(mut_repo, git_settings, |_| true)
}

/// Reflect changes made in the underlying Git repo in the Jujutsu repo.
///
/// Only branches whose git full reference name pass the filter will be
/// considered for addition, update, or deletion.
pub fn import_some_refs(
    mut_repo: &mut MutableRepo,
    git_settings: &GitSettings,
    git_ref_filter: impl Fn(&RefName) -> bool,
) -> Result<GitImportStats, GitImportError> {
    let store = mut_repo.store();
    let git_backend = get_git_backend(store).ok_or(GitImportError::UnexpectedBackend)?;
    let git_repo = git_backend.git_repo();

    let RefsToImport {
        changed_git_refs,
        changed_remote_refs,
    } = diff_refs_to_import(mut_repo.view(), &git_repo, git_ref_filter)?;

    // Bulk-import all reachable Git commits to the backend to reduce overhead
    // of table merging and ref updates.
    //
    // changed_remote_refs might contain new_targets that are not in
    // changed_git_refs, but such targets should have already been imported to
    // the backend.
    let index = mut_repo.index();
    let missing_head_ids = changed_git_refs
        .iter()
        .flat_map(|(_, new_target)| new_target.added_ids())
        .filter(|&id| !index.has_id(id));
    let heads_imported = git_backend.import_head_commits(missing_head_ids).is_ok();

    // Import new remote heads
    let mut head_commits = Vec::new();
    let get_commit = |id| {
        // If bulk-import failed, try again to find bad head or ref.
        if !heads_imported && !index.has_id(id) {
            git_backend.import_head_commits([id])?;
        }
        store.get_commit(id)
    };
    for (ref_name, (_, new_target)) in &changed_remote_refs {
        for id in new_target.added_ids() {
            let commit = get_commit(id).map_err(|err| GitImportError::MissingRefAncestor {
                ref_name: ref_name.to_string(),
                err,
            })?;
            head_commits.push(commit);
        }
    }
    // It's unlikely the imported commits were missing, but I/O-related error
    // can still occur.
    mut_repo
        .add_heads(&head_commits)
        .map_err(GitImportError::InternalBackend)?;

    // Apply the change that happened in git since last time we imported refs.
    for (full_name, new_target) in changed_git_refs {
        mut_repo.set_git_ref_target(&full_name, new_target);
    }
    for (ref_name, (old_remote_ref, new_target)) in &changed_remote_refs {
        let base_target = old_remote_ref.tracking_target();
        let new_remote_ref = RemoteRef {
            target: new_target.clone(),
            state: if old_remote_ref.is_present() {
                old_remote_ref.state
            } else {
                default_remote_ref_state_for(ref_name, git_settings)
            },
        };
        match ref_name {
            RefName::LocalBranch(branch) => {
                if new_remote_ref.is_tracking() {
                    mut_repo.merge_local_branch(branch, base_target, &new_remote_ref.target);
                }
                // Update Git-tracking branch like the other remote branches.
                mut_repo.set_remote_branch(branch, REMOTE_NAME_FOR_LOCAL_GIT_REPO, new_remote_ref);
            }
            RefName::RemoteBranch { branch, remote } => {
                if new_remote_ref.is_tracking() {
                    mut_repo.merge_local_branch(branch, base_target, &new_remote_ref.target);
                }
                // Remote-tracking branch is the last known state of the branch in the remote.
                // It shouldn't diverge even if we had inconsistent view.
                mut_repo.set_remote_branch(branch, remote, new_remote_ref);
            }
            RefName::Tag(name) => {
                if new_remote_ref.is_tracking() {
                    mut_repo.merge_tag(name, base_target, &new_remote_ref.target);
                }
                // TODO: If we add Git-tracking tag, it will be updated here.
            }
        }
    }

    let abandoned_commits = if git_settings.abandon_unreachable_commits {
        abandon_unreachable_commits(mut_repo, &changed_remote_refs)
    } else {
        vec![]
    };
    let stats = GitImportStats {
        abandoned_commits,
        changed_remote_refs,
    };
    Ok(stats)
}

/// Finds commits that used to be reachable in git that no longer are reachable.
/// Those commits will be recorded as abandoned in the `MutableRepo`.
fn abandon_unreachable_commits(
    mut_repo: &mut MutableRepo,
    changed_remote_refs: &BTreeMap<RefName, (RemoteRef, RefTarget)>,
) -> Vec<CommitId> {
    let hidable_git_heads = changed_remote_refs
        .values()
        .flat_map(|(old_remote_ref, _)| old_remote_ref.target.added_ids())
        .cloned()
        .collect_vec();
    if hidable_git_heads.is_empty() {
        return vec![];
    }
    let pinned_expression = RevsetExpression::union_all(&[
        // Local refs are usually visible, no need to filter out hidden
        RevsetExpression::commits(pinned_commit_ids(mut_repo.view())),
        RevsetExpression::commits(remotely_pinned_commit_ids(mut_repo.view()))
            // Hidden remote branches should not contribute to pinning
            .intersection(&RevsetExpression::visible_heads().ancestors()),
        RevsetExpression::root(),
    ]);
    let abandoned_expression = pinned_expression
        .range(&RevsetExpression::commits(hidable_git_heads))
        // Don't include already-abandoned commits in GitImportStats
        .intersection(&RevsetExpression::visible_heads().ancestors());
    let abandoned_commits = abandoned_expression
        .evaluate_programmatic(mut_repo)
        .unwrap()
        .iter()
        .collect_vec();
    for abandoned_commit in &abandoned_commits {
        mut_repo.record_abandoned_commit(abandoned_commit.clone());
    }
    abandoned_commits
}

/// Calculates diff of git refs to be imported.
fn diff_refs_to_import(
    view: &View,
    git_repo: &gix::Repository,
    git_ref_filter: impl Fn(&RefName) -> bool,
) -> Result<RefsToImport, GitImportError> {
    let mut known_git_refs: HashMap<&str, &RefTarget> = view
        .git_refs()
        .iter()
        .filter_map(|(full_name, target)| {
            // TODO: or clean up invalid ref in case it was stored due to historical bug?
            let ref_name = parse_git_ref(full_name).expect("stored git ref should be parsable");
            git_ref_filter(&ref_name).then_some((full_name.as_ref(), target))
        })
        .collect();
    // TODO: migrate tags to the remote view, and don't destructure &RemoteRef
    let mut known_remote_refs: HashMap<RefName, (&RefTarget, RemoteRefState)> = itertools::chain(
        view.all_remote_branches()
            .map(|((branch, remote), remote_ref)| {
                // TODO: want to abstract local ref as "git" tracking remote, but
                // we'll probably need to refactor the git_ref_filter API first.
                let ref_name = if remote == REMOTE_NAME_FOR_LOCAL_GIT_REPO {
                    RefName::LocalBranch(branch.to_owned())
                } else {
                    RefName::RemoteBranch {
                        branch: branch.to_owned(),
                        remote: remote.to_owned(),
                    }
                };
                let RemoteRef { target, state } = remote_ref;
                (ref_name, (target, *state))
            }),
        // TODO: compare to tags stored in the "git" remote view. Since tags should never
        // be moved locally in jj, we can consider local tags as merge base.
        view.tags().iter().map(|(name, target)| {
            let ref_name = RefName::Tag(name.to_owned());
            (ref_name, (target, RemoteRefState::Tracking))
        }),
    )
    .filter(|(ref_name, _)| git_ref_filter(ref_name))
    .collect();

    let mut changed_git_refs = Vec::new();
    let mut changed_remote_refs = BTreeMap::new();
    let git_references = git_repo.references().map_err(GitImportError::from_git)?;
    let chain_git_refs_iters = || -> Result<_, gix::reference::iter::init::Error> {
        // Exclude uninteresting directories such as refs/jj/keep.
        Ok(itertools::chain!(
            git_references.local_branches()?,
            git_references.remote_branches()?,
            git_references.tags()?,
        ))
    };
    for git_ref in chain_git_refs_iters().map_err(GitImportError::from_git)? {
        let git_ref = git_ref.map_err(GitImportError::from_git)?;
        let Ok(full_name) = str::from_utf8(git_ref.name().as_bstr()) else {
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
        if is_reserved_git_remote_ref(&ref_name) {
            return Err(GitImportError::RemoteReservedForLocalGitRepo);
        }
        let old_git_target = known_git_refs.get(full_name).copied().flatten();
        let Some(id) = resolve_git_ref_to_commit_id(&git_ref, old_git_target) else {
            // Skip (or remove existing) invalid refs.
            continue;
        };
        let new_target = RefTarget::normal(id);
        known_git_refs.remove(full_name);
        if new_target != *old_git_target {
            changed_git_refs.push((full_name.to_owned(), new_target.clone()));
        }
        // TODO: Make it configurable which remotes are publishing and update public
        // heads here.
        let (old_remote_target, old_remote_state) = known_remote_refs
            .remove(&ref_name)
            .unwrap_or_else(|| (RefTarget::absent_ref(), RemoteRefState::New));
        if new_target != *old_remote_target {
            let old_remote_ref = RemoteRef {
                target: old_remote_target.clone(),
                state: old_remote_state,
            };
            changed_remote_refs.insert(ref_name, (old_remote_ref, new_target));
        }
    }
    for full_name in known_git_refs.into_keys() {
        changed_git_refs.push((full_name.to_owned(), RefTarget::absent()));
    }
    for (ref_name, (old_target, old_state)) in known_remote_refs {
        let old_remote_ref = RemoteRef {
            target: old_target.clone(),
            state: old_state,
        };
        changed_remote_refs.insert(ref_name, (old_remote_ref, RefTarget::absent()));
    }
    Ok(RefsToImport {
        changed_git_refs,
        changed_remote_refs,
    })
}

fn default_remote_ref_state_for(ref_name: &RefName, git_settings: &GitSettings) -> RemoteRefState {
    match ref_name {
        // LocalBranch means Git-tracking branch
        RefName::LocalBranch(_) | RefName::Tag(_) => RemoteRefState::Tracking,
        RefName::RemoteBranch { .. } => {
            if git_settings.auto_local_branch {
                RemoteRefState::Tracking
            } else {
                RemoteRefState::New
            }
        }
    }
}

/// Commits referenced by local branches or tags.
///
/// On `import_refs()`, this is similar to collecting commits referenced by
/// `view.git_refs()`. Main difference is that local branches can be moved by
/// tracking remotes, and such mutation isn't applied to `view.git_refs()` yet.
fn pinned_commit_ids(view: &View) -> Vec<CommitId> {
    itertools::chain(
        view.local_branches().map(|(_, target)| target),
        view.tags().values(),
    )
    .flat_map(|target| target.added_ids())
    .cloned()
    .collect()
}

/// Commits referenced by untracked remote branches including hidden ones.
///
/// Tracked remote branches aren't included because they should have been merged
/// into the local counterparts, and the changes pulled from one remote should
/// propagate to the other remotes on later push. OTOH, untracked remote
/// branches are considered independent refs.
fn remotely_pinned_commit_ids(view: &View) -> Vec<CommitId> {
    view.all_remote_branches()
        .filter(|(_, remote_ref)| !remote_ref.is_tracking())
        .map(|(_, remote_ref)| &remote_ref.target)
        .flat_map(|target| target.added_ids())
        .cloned()
        .collect()
}

/// Imports `HEAD@git` from the underlying Git repo.
///
/// Unlike `import_refs()`, the old HEAD branch is not abandoned because HEAD
/// move doesn't always mean the old HEAD branch has been rewritten.
///
/// Unlike `reset_head()`, this function doesn't move the working-copy commit to
/// the child of the new `HEAD@git` revision.
pub fn import_head(mut_repo: &mut MutableRepo) -> Result<(), GitImportError> {
    let store = mut_repo.store();
    let git_backend = get_git_backend(store).ok_or(GitImportError::UnexpectedBackend)?;
    let git_repo = git_backend.git_repo();

    let old_git_head = mut_repo.view().git_head();
    let new_git_head_id = if let Ok(oid) = git_repo.head_id() {
        Some(CommitId::from_bytes(oid.as_bytes()))
    } else {
        None
    };
    if old_git_head.as_resolved() == Some(&new_git_head_id) {
        return Ok(());
    }

    // Import new head
    if let Some(head_id) = &new_git_head_id {
        let index = mut_repo.index();
        if !index.has_id(head_id) {
            git_backend.import_head_commits([head_id]).map_err(|err| {
                GitImportError::MissingHeadTarget {
                    id: head_id.clone(),
                    err,
                }
            })?;
        }
        // It's unlikely the imported commits were missing, but I/O-related
        // error can still occur.
        store
            .get_commit(head_id)
            .and_then(|commit| mut_repo.add_head(&commit))
            .map_err(GitImportError::InternalBackend)?;
    }

    mut_repo.set_git_head_target(RefTarget::resolved(new_git_head_id));
    Ok(())
}

#[derive(Error, Debug)]
pub enum GitExportError {
    #[error("Git error")]
    InternalGitError(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("The repo is not backed by a Git repo")]
    UnexpectedBackend,
}

impl GitExportError {
    fn from_git(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        GitExportError::InternalGitError(source.into())
    }
}

/// A ref we failed to export to Git, along with the reason it failed.
#[derive(Debug)]
pub struct FailedRefExport {
    pub name: RefName,
    pub reason: FailedRefExportReason,
}

/// The reason we failed to export a ref to Git.
#[derive(Debug, Error)]
pub enum FailedRefExportReason {
    /// The name is not allowed in Git.
    #[error("Name is not allowed in Git")]
    InvalidGitName,
    /// The ref was in a conflicted state from the last import. A re-import
    /// should fix it.
    #[error("Ref was in a conflicted state from the last import")]
    ConflictedOldState,
    /// The branch points to the root commit, which Git doesn't have
    #[error("Ref cannot point to the root commit in Git")]
    OnRootCommit,
    /// We wanted to delete it, but it had been modified in Git.
    #[error("Deleted ref had been modified in Git")]
    DeletedInJjModifiedInGit,
    /// We wanted to add it, but Git had added it with a different target
    #[error("Added ref had been added with a different target in Git")]
    AddedInJjAddedInGit,
    /// We wanted to modify it, but Git had deleted it
    #[error("Modified ref had been deleted in Git")]
    ModifiedInJjDeletedInGit,
    /// Failed to delete the ref from the Git repo
    #[error("Failed to delete")]
    FailedToDelete(#[source] Box<gix::reference::edit::Error>),
    /// Failed to set the ref in the Git repo
    #[error("Failed to set")]
    FailedToSet(#[source] Box<gix::reference::edit::Error>),
}

#[derive(Debug)]
struct RefsToExport {
    branches_to_update: BTreeMap<RefName, (Option<gix::ObjectId>, gix::ObjectId)>,
    branches_to_delete: BTreeMap<RefName, gix::ObjectId>,
    failed_branches: HashMap<RefName, FailedRefExportReason>,
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
pub fn export_refs(mut_repo: &mut MutableRepo) -> Result<Vec<FailedRefExport>, GitExportError> {
    export_some_refs(mut_repo, |_| true)
}

pub fn export_some_refs(
    mut_repo: &mut MutableRepo,
    git_ref_filter: impl Fn(&RefName) -> bool,
) -> Result<Vec<FailedRefExport>, GitExportError> {
    let git_repo = get_git_repo(mut_repo.store()).ok_or(GitExportError::UnexpectedBackend)?;

    let RefsToExport {
        branches_to_update,
        branches_to_delete,
        mut failed_branches,
    } = diff_refs_to_export(
        mut_repo.view(),
        mut_repo.store().root_commit_id(),
        &git_ref_filter,
    );

    // TODO: Also check other worktrees' HEAD.
    if let Ok(head_ref) = git_repo.find_reference("HEAD") {
        if let Some(parsed_ref) = head_ref
            .target()
            .try_name()
            .and_then(|name| str::from_utf8(name.as_bstr()).ok())
            .and_then(parse_git_ref)
        {
            let old_target = head_ref.inner.target.clone();
            let current_oid = match head_ref.into_fully_peeled_id() {
                Ok(id) => Some(id.detach()),
                Err(gix::reference::peel::Error::ToId(gix::refs::peel::to_id::Error::Follow(
                    gix::refs::file::find::existing::Error::NotFound { .. },
                ))) => None, // Unborn ref should be considered absent
                Err(err) => return Err(GitExportError::from_git(err)),
            };
            let new_oid = if let Some((_old_oid, new_oid)) = branches_to_update.get(&parsed_ref) {
                Some(new_oid)
            } else if branches_to_delete.contains_key(&parsed_ref) {
                None
            } else {
                current_oid.as_ref()
            };
            if new_oid != current_oid.as_ref() {
                update_git_head(&git_repo, old_target, current_oid)?;
            }
        }
    }
    for (parsed_ref_name, old_oid) in branches_to_delete {
        let Some(git_ref_name) = to_git_ref_name(&parsed_ref_name) else {
            failed_branches.insert(parsed_ref_name, FailedRefExportReason::InvalidGitName);
            continue;
        };
        if let Err(reason) = delete_git_ref(&git_repo, &git_ref_name, &old_oid) {
            failed_branches.insert(parsed_ref_name, reason);
        } else {
            let new_target = RefTarget::absent();
            mut_repo.set_git_ref_target(&git_ref_name, new_target);
        }
    }
    for (parsed_ref_name, (old_oid, new_oid)) in branches_to_update {
        let Some(git_ref_name) = to_git_ref_name(&parsed_ref_name) else {
            failed_branches.insert(parsed_ref_name, FailedRefExportReason::InvalidGitName);
            continue;
        };
        if let Err(reason) = update_git_ref(&git_repo, &git_ref_name, old_oid, new_oid) {
            failed_branches.insert(parsed_ref_name, reason);
        } else {
            let new_target = RefTarget::normal(CommitId::from_bytes(new_oid.as_bytes()));
            mut_repo.set_git_ref_target(&git_ref_name, new_target);
        }
    }

    copy_exportable_local_branches_to_remote_view(
        mut_repo,
        REMOTE_NAME_FOR_LOCAL_GIT_REPO,
        |ref_name| git_ref_filter(ref_name) && !failed_branches.contains_key(ref_name),
    );

    let failed_branches = failed_branches
        .into_iter()
        .map(|(name, reason)| FailedRefExport { name, reason })
        .sorted_unstable_by(|a, b| a.name.cmp(&b.name))
        .collect();
    Ok(failed_branches)
}

fn copy_exportable_local_branches_to_remote_view(
    mut_repo: &mut MutableRepo,
    remote_name: &str,
    git_ref_filter: impl Fn(&RefName) -> bool,
) {
    let new_local_branches = mut_repo
        .view()
        .local_remote_branches(remote_name)
        .filter_map(|(branch, targets)| {
            // TODO: filter out untracked branches (if we add support for untracked @git
            // branches)
            let old_target = &targets.remote_ref.target;
            let new_target = targets.local_target;
            (!new_target.has_conflict() && old_target != new_target).then_some((branch, new_target))
        })
        .filter(|&(branch, _)| git_ref_filter(&RefName::LocalBranch(branch.to_owned())))
        .map(|(branch, new_target)| (branch.to_owned(), new_target.clone()))
        .collect_vec();
    for (branch, new_target) in new_local_branches {
        let new_remote_ref = RemoteRef {
            target: new_target,
            state: RemoteRefState::Tracking,
        };
        mut_repo.set_remote_branch(&branch, remote_name, new_remote_ref);
    }
}

/// Calculates diff of branches to be exported.
fn diff_refs_to_export(
    view: &View,
    root_commit_id: &CommitId,
    git_ref_filter: impl Fn(&RefName) -> bool,
) -> RefsToExport {
    // Local targets will be copied to the "git" remote if successfully exported. So
    // the local branches are considered to be the new "git" remote branches.
    let mut all_branch_targets: HashMap<RefName, (&RefTarget, &RefTarget)> = itertools::chain(
        view.local_branches()
            .map(|(branch, target)| (RefName::LocalBranch(branch.to_owned()), target)),
        view.all_remote_branches()
            .filter(|&((_, remote), _)| remote != REMOTE_NAME_FOR_LOCAL_GIT_REPO)
            .map(|((branch, remote), remote_ref)| {
                let ref_name = RefName::RemoteBranch {
                    branch: branch.to_owned(),
                    remote: remote.to_owned(),
                };
                (ref_name, &remote_ref.target)
            }),
    )
    .map(|(ref_name, new_target)| (ref_name, (RefTarget::absent_ref(), new_target)))
    .filter(|(ref_name, _)| git_ref_filter(ref_name))
    .collect();
    let known_git_refs = view
        .git_refs()
        .iter()
        .map(|(full_name, target)| {
            let ref_name = parse_git_ref(full_name).expect("stored git ref should be parsable");
            (ref_name, target)
        })
        .filter(|(ref_name, _)| {
            // There are two situations where remote-tracking branches get out of sync:
            // 1. `jj branch forget`
            // 2. `jj op undo`/`restore` in colocated repo
            matches!(
                ref_name,
                RefName::LocalBranch(..) | RefName::RemoteBranch { .. }
            )
        })
        .filter(|(ref_name, _)| git_ref_filter(ref_name));
    for (ref_name, target) in known_git_refs {
        all_branch_targets
            .entry(ref_name)
            .and_modify(|(old_target, _)| *old_target = target)
            .or_insert((target, RefTarget::absent_ref()));
    }

    let mut branches_to_update = BTreeMap::new();
    let mut branches_to_delete = BTreeMap::new();
    let mut failed_branches = HashMap::new();
    let root_commit_target = RefTarget::normal(root_commit_id.clone());
    for (ref_name, (old_target, new_target)) in all_branch_targets {
        if new_target == old_target {
            continue;
        }
        if *new_target == root_commit_target {
            // Git doesn't have a root commit
            failed_branches.insert(ref_name, FailedRefExportReason::OnRootCommit);
            continue;
        }
        let old_oid = if let Some(id) = old_target.as_normal() {
            Some(gix::ObjectId::try_from(id.as_bytes()).unwrap())
        } else if old_target.has_conflict() {
            // The old git ref should only be a conflict if there were concurrent import
            // operations while the value changed. Don't overwrite these values.
            failed_branches.insert(ref_name, FailedRefExportReason::ConflictedOldState);
            continue;
        } else {
            assert!(old_target.is_absent());
            None
        };
        if let Some(id) = new_target.as_normal() {
            let new_oid = gix::ObjectId::try_from(id.as_bytes()).unwrap();
            branches_to_update.insert(ref_name, (old_oid, new_oid));
        } else if new_target.has_conflict() {
            // Skip conflicts and leave the old value in git_refs
            continue;
        } else {
            assert!(new_target.is_absent());
            branches_to_delete.insert(ref_name, old_oid.unwrap());
        }
    }

    RefsToExport {
        branches_to_update,
        branches_to_delete,
        failed_branches,
    }
}

fn delete_git_ref(
    git_repo: &gix::Repository,
    git_ref_name: &str,
    old_oid: &gix::oid,
) -> Result<(), FailedRefExportReason> {
    if let Ok(git_ref) = git_repo.find_reference(git_ref_name) {
        if git_ref.inner.target.try_id() == Some(old_oid) {
            // The branch has not been updated by git, so go ahead and delete it
            git_ref
                .delete()
                .map_err(|err| FailedRefExportReason::FailedToDelete(err.into()))?;
        } else {
            // The branch was updated by git
            return Err(FailedRefExportReason::DeletedInJjModifiedInGit);
        }
    } else {
        // The branch is already deleted
    }
    Ok(())
}

fn update_git_ref(
    git_repo: &gix::Repository,
    git_ref_name: &str,
    old_oid: Option<gix::ObjectId>,
    new_oid: gix::ObjectId,
) -> Result<(), FailedRefExportReason> {
    match old_oid {
        None => {
            if let Ok(git_repo_ref) = git_repo.find_reference(git_ref_name) {
                // The branch was added in jj and in git. We're good if and only if git
                // pointed it to our desired target.
                if git_repo_ref.inner.target.try_id() != Some(&new_oid) {
                    return Err(FailedRefExportReason::AddedInJjAddedInGit);
                }
            } else {
                // The branch was added in jj but still doesn't exist in git, so add it
                git_repo
                    .reference(
                        git_ref_name,
                        new_oid,
                        gix::refs::transaction::PreviousValue::MustNotExist,
                        "export from jj",
                    )
                    .map_err(|err| FailedRefExportReason::FailedToSet(err.into()))?;
            }
        }
        Some(old_oid) => {
            // The branch was modified in jj. We can use gix API for updating under a lock.
            if let Err(err) = git_repo.reference(
                git_ref_name,
                new_oid,
                gix::refs::transaction::PreviousValue::MustExistAndMatch(old_oid.into()),
                "export from jj",
            ) {
                // The reference was probably updated in git
                if let Ok(git_repo_ref) = git_repo.find_reference(git_ref_name) {
                    // We still consider this a success if it was updated to our desired target
                    if git_repo_ref.inner.target.try_id() != Some(&new_oid) {
                        return Err(FailedRefExportReason::FailedToSet(err.into()));
                    }
                } else {
                    // The reference was deleted in git and moved in jj
                    return Err(FailedRefExportReason::ModifiedInJjDeletedInGit);
                }
            } else {
                // Successfully updated from old_oid to new_oid (unchanged in
                // git)
            }
        }
    }
    Ok(())
}

/// Ensures `HEAD@git` is detached and pointing to the `new_oid`. If `new_oid`
/// is `None` (meaning absent), dummy placeholder ref will be set.
fn update_git_head(
    git_repo: &gix::Repository,
    old_target: gix::refs::Target,
    new_oid: Option<gix::ObjectId>,
) -> Result<(), GitExportError> {
    let mut ref_edits = Vec::new();
    let new_target = if let Some(oid) = new_oid {
        gix::refs::Target::Peeled(oid)
    } else {
        // Can't detach HEAD without a commit. Use placeholder ref to nullify
        // the HEAD. The placeholder ref isn't a normal branch ref. Git CLI
        // appears to deal with that, and can move the placeholder ref. So we
        // need to ensure that the ref doesn't exist.
        ref_edits.push(gix::refs::transaction::RefEdit {
            change: gix::refs::transaction::Change::Delete {
                expected: gix::refs::transaction::PreviousValue::Any,
                log: gix::refs::transaction::RefLog::AndReference,
            },
            name: UNBORN_ROOT_REF_NAME.try_into().unwrap(),
            deref: false,
        });
        gix::refs::Target::Symbolic(UNBORN_ROOT_REF_NAME.try_into().unwrap())
    };
    ref_edits.push(gix::refs::transaction::RefEdit {
        change: gix::refs::transaction::Change::Update {
            log: gix::refs::transaction::LogChange {
                message: "export from jj".into(),
                ..Default::default()
            },
            expected: gix::refs::transaction::PreviousValue::MustExistAndMatch(old_target),
            new: new_target,
        },
        name: "HEAD".try_into().unwrap(),
        deref: false,
    });
    git_repo
        .edit_references(ref_edits)
        .map_err(GitExportError::from_git)?;
    Ok(())
}

/// Sets `HEAD@git` to the parent of the given working-copy commit and resets
/// the Git index.
pub fn reset_head(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
    wc_commit: &Commit,
) -> Result<(), git2::Error> {
    let first_parent_id = &wc_commit.parent_ids()[0];
    let first_parent = if first_parent_id != mut_repo.store().root_commit_id() {
        RefTarget::normal(first_parent_id.clone())
    } else {
        RefTarget::absent()
    };
    if first_parent.is_present() {
        let git_head = mut_repo.view().git_head();
        let new_git_commit_id = Oid::from_bytes(first_parent_id.as_bytes()).unwrap();
        let new_git_commit = git_repo.find_commit(new_git_commit_id)?;
        if git_head != &first_parent {
            git_repo.set_head_detached(new_git_commit_id)?;
        }

        let is_same_tree = if git_head == &first_parent {
            true
        } else if let Some(git_head_id) = git_head.as_normal() {
            let git_head_oid = Oid::from_bytes(git_head_id.as_bytes()).unwrap();
            let git_head_commit = git_repo.find_commit(git_head_oid)?;
            new_git_commit.tree_id() == git_head_commit.tree_id()
        } else {
            false
        };
        let skip_reset = if is_same_tree {
            // `HEAD@git` already points to a commit with the correct tree contents,
            // so we only need to reset the Git index. We can skip the reset if
            // the Git index is empty (i.e. `git add` was never used).
            // In large repositories, this is around 2x faster if the Git index is empty
            // (~0.89s to check the diff, vs. ~1.72s to reset), and around 8% slower if
            // it isn't (~1.86s to check the diff AND reset).
            let diff = git_repo.diff_tree_to_index(
                Some(&new_git_commit.tree()?),
                None,
                Some(git2::DiffOptions::new().skip_binary_check(true)),
            )?;
            diff.deltas().len() == 0
        } else {
            false
        };
        if !skip_reset {
            git_repo.reset(new_git_commit.as_object(), git2::ResetType::Mixed, None)?;
        }
    } else {
        // Can't detach HEAD without a commit. Use placeholder ref to nullify the HEAD.
        // We can't set_head() an arbitrary unborn ref, so use reference_symbolic()
        // instead. Git CLI appears to deal with that. It would be nice if Git CLI
        // couldn't create a commit without setting a valid branch name.
        if mut_repo.git_head().is_present() {
            match git_repo.find_reference(UNBORN_ROOT_REF_NAME) {
                Ok(mut git_repo_ref) => git_repo_ref.delete()?,
                Err(err) if err.code() == git2::ErrorCode::NotFound => {}
                Err(err) => return Err(err),
            }
            git_repo.reference_symbolic("HEAD", UNBORN_ROOT_REF_NAME, true, "unset HEAD by jj")?;
        }
        // git_reset() of libgit2 requires a commit object. Do that manually.
        let mut index = git_repo.index()?;
        index.clear()?; // or read empty tree
        index.write()?;
        git_repo.cleanup_state()?;
    }
    mut_repo.set_git_head_target(first_parent);
    Ok(())
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
    if remote_name != REMOTE_NAME_FOR_LOCAL_GIT_REPO {
        remove_remote_refs(mut_repo, remote_name);
    }
    Ok(())
}

fn remove_remote_refs(mut_repo: &mut MutableRepo, remote_name: &str) {
    mut_repo.remove_remote(remote_name);
    let prefix = format!("refs/remotes/{remote_name}/");
    let git_refs_to_delete = mut_repo
        .view()
        .git_refs()
        .keys()
        .filter(|&r| r.starts_with(&prefix))
        .cloned()
        .collect_vec();
    for git_ref in git_refs_to_delete {
        mut_repo.set_git_ref_target(&git_ref, RefTarget::absent());
    }
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
    if old_remote_name != REMOTE_NAME_FOR_LOCAL_GIT_REPO {
        rename_remote_refs(mut_repo, old_remote_name, new_remote_name);
    }
    Ok(())
}

pub fn set_remote_url(
    git_repo: &git2::Repository,
    remote_name: &str,
    new_remote_url: &str,
) -> Result<(), GitRemoteManagementError> {
    if remote_name == REMOTE_NAME_FOR_LOCAL_GIT_REPO {
        return Err(GitRemoteManagementError::RemoteReservedForLocalGitRepo);
    }

    // Repository::remote_set_url() doesn't ensure the remote exists, it just
    // creates it if it's missing.
    // Therefore ensure it exists first
    git_repo.find_remote(remote_name).map_err(|err| {
        if is_remote_not_found_err(&err) {
            GitRemoteManagementError::NoSuchRemote(remote_name.to_owned())
        } else {
            GitRemoteManagementError::InternalGitError(err)
        }
    })?;

    git_repo
        .remote_set_url(remote_name, new_remote_url)
        .map_err(GitRemoteManagementError::InternalGitError)?;
    Ok(())
}

fn rename_remote_refs(mut_repo: &mut MutableRepo, old_remote_name: &str, new_remote_name: &str) {
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
}

const INVALID_REFSPEC_CHARS: [char; 5] = [':', '^', '?', '[', ']'];

#[derive(Error, Debug)]
pub enum GitFetchError {
    #[error("No git remote named '{0}'")]
    NoSuchRemote(String),
    #[error(
        "Invalid branch pattern provided. Patterns may not contain the characters `{chars}`",
        chars = INVALID_REFSPEC_CHARS.iter().join("`, `")
    )]
    InvalidBranchPattern,
    #[error("Failed to import Git refs")]
    GitImportError(#[from] GitImportError),
    // TODO: I'm sure there are other errors possible, such as transport-level errors.
    #[error("Unexpected git error when fetching")]
    InternalGitError(#[from] git2::Error),
}

/// Describes successful `fetch()` result.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct GitFetchStats {
    /// Remote's default branch.
    pub default_branch: Option<String>,
    /// Changes made by the import.
    pub import_stats: GitImportStats,
}

#[tracing::instrument(skip(mut_repo, git_repo, callbacks))]
pub fn fetch(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
    remote_name: &str,
    branch_names: &[StringPattern],
    callbacks: RemoteCallbacks<'_>,
    git_settings: &GitSettings,
) -> Result<GitFetchStats, GitFetchError> {
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
    // At this point, we are only updating Git's remote tracking branches, not the
    // local branches.
    let refspecs: Vec<_> = branch_names
        .iter()
        .map(|pattern| {
            pattern
                .to_glob()
                .filter(|glob| !glob.contains(INVALID_REFSPEC_CHARS))
                .map(|glob| format!("+refs/heads/{glob}:refs/remotes/{remote_name}/{glob}"))
        })
        .collect::<Option<_>>()
        .ok_or(GitFetchError::InvalidBranchPattern)?;
    if refspecs.is_empty() {
        // Don't fall back to the base refspecs.
        let stats = GitFetchStats::default();
        return Ok(stats);
    }
    tracing::debug!("remote.download");
    remote.download(&refspecs, Some(&mut fetch_options))?;
    tracing::debug!("remote.prune");
    remote.prune(None)?;
    tracing::debug!("remote.update_tips");
    remote.update_tips(
        None,
        git2::RemoteUpdateFlags::empty(),
        git2::AutotagOption::Unspecified,
        None,
    )?;
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

    // Import the remote-tracking branches into the jj repo and update jj's
    // local branches. We also import local tags since remote tags should have
    // been merged by Git.
    tracing::debug!("import_refs");
    let import_stats = import_some_refs(mut_repo, git_settings, |ref_name| {
        to_remote_branch(ref_name, remote_name)
            .map(|branch| branch_names.iter().any(|pattern| pattern.matches(branch)))
            .unwrap_or_else(|| matches!(ref_name, RefName::Tag(_)))
    })?;
    let stats = GitFetchStats {
        default_branch,
        import_stats,
    };
    Ok(stats)
}

#[derive(Error, Debug, PartialEq)]
pub enum GitPushError {
    #[error("No git remote named '{0}'")]
    NoSuchRemote(String),
    #[error(
        "Git remote named '{name}' is reserved for local Git repository",
        name = REMOTE_NAME_FOR_LOCAL_GIT_REPO
    )]
    RemoteReservedForLocalGitRepo,
    #[error("Refs in unexpected location: {0:?}")]
    RefInUnexpectedLocation(Vec<String>),
    #[error("Remote rejected the update of some refs (do you have permission to push to {0:?}?)")]
    RefUpdateRejected(Vec<String>),
    // TODO: I'm sure there are other errors possible, such as transport-level errors,
    // and errors caused by the remote rejecting the push.
    #[error("Unexpected git error when pushing")]
    InternalGitError(#[from] git2::Error),
}

#[derive(Clone, Debug)]
pub struct GitBranchPushTargets {
    pub branch_updates: Vec<(String, BranchPushUpdate)>,
}

pub struct GitRefUpdate {
    pub qualified_name: String,
    /// Expected position on the remote or None if we expect the ref to not
    /// exist on the remote
    ///
    /// This is sourced from the local remote-tracking branch.
    pub expected_current_target: Option<CommitId>,
    pub new_target: Option<CommitId>,
}

/// Pushes the specified branches and updates the repo view accordingly.
pub fn push_branches(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
    remote_name: &str,
    targets: &GitBranchPushTargets,
    callbacks: RemoteCallbacks<'_>,
) -> Result<(), GitPushError> {
    let ref_updates = targets
        .branch_updates
        .iter()
        .map(|(branch_name, update)| GitRefUpdate {
            qualified_name: format!("refs/heads/{branch_name}"),
            expected_current_target: update.old_target.clone(),
            new_target: update.new_target.clone(),
        })
        .collect_vec();
    push_updates(mut_repo, git_repo, remote_name, &ref_updates, callbacks)?;

    // TODO: add support for partially pushed refs? we could update the view
    // excluding rejected refs, but the transaction would be aborted anyway
    // if we returned an Err.
    for (branch_name, update) in &targets.branch_updates {
        let git_ref_name = format!("refs/remotes/{remote_name}/{branch_name}");
        let new_remote_ref = RemoteRef {
            target: RefTarget::resolved(update.new_target.clone()),
            state: RemoteRefState::Tracking,
        };
        mut_repo.set_git_ref_target(&git_ref_name, new_remote_ref.target.clone());
        mut_repo.set_remote_branch(branch_name, remote_name, new_remote_ref);
    }

    Ok(())
}

/// Pushes the specified Git refs without updating the repo view.
pub fn push_updates(
    repo: &dyn Repo,
    git_repo: &git2::Repository,
    remote_name: &str,
    updates: &[GitRefUpdate],
    callbacks: RemoteCallbacks<'_>,
) -> Result<(), GitPushError> {
    let mut qualified_remote_refs_expected_locations = HashMap::new();
    let mut refspecs = vec![];
    for update in updates {
        qualified_remote_refs_expected_locations.insert(
            update.qualified_name.as_str(),
            update.expected_current_target.as_ref(),
        );
        if let Some(new_target) = &update.new_target {
            // We always force-push. We use the push_negotiation callback in
            // `push_refs` to check that the refs did not unexpectedly move on
            // the remote.
            refspecs.push(format!("+{}:{}", new_target.hex(), update.qualified_name));
        } else {
            // Prefixing this with `+` to force-push or not should make no
            // difference. The push negotiation happens regardless, and wouldn't
            // allow creating a branch if it's not a fast-forward.
            refspecs.push(format!(":{}", update.qualified_name));
        }
    }
    // TODO(ilyagr): `push_refs`, or parts of it, should probably be inlined. This
    // requires adjusting some tests.
    push_refs(
        repo,
        git_repo,
        remote_name,
        &qualified_remote_refs_expected_locations,
        &refspecs,
        callbacks,
    )
}

fn push_refs(
    repo: &dyn Repo,
    git_repo: &git2::Repository,
    remote_name: &str,
    qualified_remote_refs_expected_locations: &HashMap<&str, Option<&CommitId>>,
    refspecs: &[String],
    callbacks: RemoteCallbacks<'_>,
) -> Result<(), GitPushError> {
    if remote_name == REMOTE_NAME_FOR_LOCAL_GIT_REPO {
        return Err(GitPushError::RemoteReservedForLocalGitRepo);
    }
    let mut remote = git_repo.find_remote(remote_name).map_err(|err| {
        if is_remote_not_found_err(&err) {
            GitPushError::NoSuchRemote(remote_name.to_string())
        } else {
            GitPushError::InternalGitError(err)
        }
    })?;
    let mut remaining_remote_refs: HashSet<_> = qualified_remote_refs_expected_locations
        .keys()
        .copied()
        .collect();
    let mut failed_push_negotiations = vec![];
    let push_result = {
        let mut push_options = git2::PushOptions::new();
        let mut proxy_options = git2::ProxyOptions::new();
        proxy_options.auto();
        push_options.proxy_options(proxy_options);
        let mut callbacks = callbacks.into_git();
        callbacks.push_negotiation(|updates| {
            for update in updates {
                let dst_refname = update
                    .dst_refname()
                    .expect("Expect reference name to be valid UTF-8");
                let expected_remote_location = *qualified_remote_refs_expected_locations
                    .get(dst_refname)
                    .expect("Push is trying to move a ref it wasn't asked to move");
                let oid_to_maybe_commitid =
                    |oid: git2::Oid| (!oid.is_zero()).then(|| CommitId::from_bytes(oid.as_bytes()));
                let actual_remote_location = oid_to_maybe_commitid(update.src());
                let local_location = oid_to_maybe_commitid(update.dst());

                match allow_push(
                    repo.index(),
                    actual_remote_location.as_ref(),
                    expected_remote_location,
                    local_location.as_ref(),
                ) {
                    Ok(PushAllowReason::NormalMatch) => {}
                    Ok(PushAllowReason::UnexpectedNoop) => {
                        tracing::info!(
                            "The push of {dst_refname} is unexpectedly a no-op, the remote branch \
                             is already at {actual_remote_location:?}. We expected it to be at \
                             {expected_remote_location:?}. We don't consider this an error.",
                        );
                    }
                    Ok(PushAllowReason::ExceptionalFastforward) => {
                        // TODO(ilyagr): We could consider printing a user-facing message at
                        // this point.
                        tracing::info!(
                            "We allow the push of {dst_refname} to {local_location:?}, even \
                             though it is unexpectedly at {actual_remote_location:?} on the \
                             server rather than the expected {expected_remote_location:?}. The \
                             desired location is a descendant of the actual location, and the \
                             actual location is a descendant of the expected location.",
                        );
                    }
                    Err(()) => {
                        // While we show debug info in the message with `--debug`,
                        // there's probably no need to show the detailed commit
                        // locations to the user normally. They should do a `jj git
                        // fetch`, and the resulting branch conflicts should contain
                        // all the information they need.
                        tracing::info!(
                            "Cannot push {dst_refname} to {local_location:?}; it is at \
                             unexpectedly at {actual_remote_location:?} on the server as opposed \
                             to the expected {expected_remote_location:?}",
                        );

                        failed_push_negotiations.push(dst_refname.to_string());
                    }
                }
            }
            if failed_push_negotiations.is_empty() {
                Ok(())
            } else {
                Err(git2::Error::from_str("failed push negotiation"))
            }
        });
        callbacks.push_update_reference(|refname, status| {
            // The status is Some if the ref update was rejected
            if status.is_none() {
                remaining_remote_refs.remove(refname);
            }
            Ok(())
        });
        push_options.remote_callbacks(callbacks);
        remote.push(refspecs, Some(&mut push_options))
    };
    if !failed_push_negotiations.is_empty() {
        // If the push negotiation returned an error, `remote.push` would not
        // have pushed anything and would have returned an error, as expected.
        // However, the error it returns is not necessarily the error we'd
        // expect. It also depends on the exact versions of `libgit2` and
        // `git2.rs`. So, we cannot rely on it containing any useful
        // information. See https://github.com/rust-lang/git2-rs/issues/1042.
        assert!(push_result.is_err());
        failed_push_negotiations.sort();
        Err(GitPushError::RefInUnexpectedLocation(
            failed_push_negotiations,
        ))
    } else {
        push_result?;
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PushAllowReason {
    NormalMatch,
    ExceptionalFastforward,
    UnexpectedNoop,
}

fn allow_push(
    index: &dyn Index,
    actual_remote_location: Option<&CommitId>,
    expected_remote_location: Option<&CommitId>,
    destination_location: Option<&CommitId>,
) -> Result<PushAllowReason, ()> {
    if actual_remote_location == expected_remote_location {
        return Ok(PushAllowReason::NormalMatch);
    }

    // If the remote ref is in an unexpected location, we still allow some
    // pushes, based on whether `jj git fetch` would result in a conflicted ref.
    //
    // For `merge_ref_targets` to work correctly, `actual_remote_location` must
    // be a commit that we locally know about.
    //
    // This does not lose any generality since for `merge_ref_targets` to
    // resolve to `local_target` below, it is conceptually necessary (but not
    // sufficient) for the destination_location to be either a descendant of
    // actual_remote_location or equal to it. Either way, we would know about that
    // commit locally.
    if !actual_remote_location.map_or(true, |id| index.has_id(id)) {
        return Err(());
    }
    let remote_target = RefTarget::resolved(actual_remote_location.cloned());
    let base_target = RefTarget::resolved(expected_remote_location.cloned());
    // The push destination is the local position of the ref
    let local_target = RefTarget::resolved(destination_location.cloned());
    if refs::merge_ref_targets(index, &remote_target, &base_target, &local_target) == local_target {
        // Fetch would not change the local branch, so the push is OK in spite of
        // the discrepancy with the expected location. We return some debug info and
        // verify some invariants before OKing the push.
        Ok(if actual_remote_location == destination_location {
            // This is the situation of what we call "A - B + A = A"
            // conflicts, see also test_refs.rs and
            // https://github.com/martinvonz/jj/blob/c9b44f382824301e6c0fdd6f4cbc52bb00c50995/lib/src/merge.rs#L92.
            PushAllowReason::UnexpectedNoop
        } else {
            // Due to our ref merge rules, this case should happen if an only
            // if:
            //
            // 1. This is a fast-forward.
            // 2. The expected location is an ancestor of both the actual location and the
            //    destination (local position).
            PushAllowReason::ExceptionalFastforward
        })
    } else {
        Err(())
    }
}

#[non_exhaustive]
#[derive(Default)]
#[allow(clippy::type_complexity)]
pub struct RemoteCallbacks<'a> {
    pub progress: Option<&'a mut dyn FnMut(&Progress)>,
    pub sideband_progress: Option<&'a mut dyn FnMut(&[u8])>,
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
        if let Some(sideband_progress_cb) = self.sideband_progress {
            callbacks.sideband_progress(move |data| {
                sideband_progress_cb(data);
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
    #[error("Unexpected io error when parsing config")]
    IoError(#[from] std::io::Error),
    #[error("Unexpected git error when parsing config")]
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
