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

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::default::Default;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use git2::Oid;
use itertools::Itertools;
use thiserror::Error;

use crate::backend::CommitId;
use crate::commit::Commit;
use crate::op_store::{BranchTarget, OperationId, RefTarget};
use crate::repo::{MutableRepo, ReadonlyRepo};
use crate::view::{RefName, View};
use crate::{op_store, simple_op_store, simple_op_store_model};

#[derive(Error, Debug, PartialEq)]
pub enum GitImportError {
    #[error("Unexpected git error when importing refs: {0}")]
    InternalGitError(#[from] git2::Error),
}

fn parse_git_ref(ref_name: &str) -> Option<RefName> {
    if let Some(branch_name) = ref_name.strip_prefix("refs/heads/") {
        Some(RefName::LocalBranch(branch_name.to_string()))
    } else if let Some(remote_and_branch) = ref_name.strip_prefix("refs/remotes/") {
        remote_and_branch
            .split_once('/')
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

/// Reflect changes made in the underlying Git repo in the Jujutsu repo.
pub fn import_refs(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
) -> Result<(), GitImportError> {
    let store = mut_repo.store().clone();
    let mut existing_git_refs = mut_repo.view().git_refs().clone();
    let mut old_git_heads = existing_git_refs
        .values()
        .flat_map(|old_target| old_target.adds())
        .collect_vec();
    if let Some(old_git_head) = mut_repo.view().git_head() {
        old_git_heads.push(old_git_head);
    }

    let mut new_git_heads = HashSet::new();
    // TODO: Should this be a separate function? We may not always want to import
    // the Git HEAD (and add it to our set of heads).
    if let Ok(head_git_commit) = git_repo
        .head()
        .and_then(|head_ref| head_ref.peel_to_commit())
    {
        let head_commit_id = CommitId::from_bytes(head_git_commit.id().as_bytes());
        let head_commit = store.get_commit(&head_commit_id).unwrap();
        new_git_heads.insert(head_commit_id.clone());
        mut_repo.add_head(&head_commit);
        mut_repo.set_git_head(head_commit_id);
    } else {
        mut_repo.clear_git_head();
    }

    let mut changed_git_refs = BTreeMap::new();
    let git_refs = git_repo.references()?;
    for git_ref in git_refs {
        let git_ref = git_ref?;
        if !(git_ref.is_tag() || git_ref.is_branch() || git_ref.is_remote())
            || git_ref.name().is_none()
        {
            // Skip other refs (such as notes) and symbolic refs, as well as non-utf8 refs.
            continue;
        }
        let full_name = git_ref.name().unwrap().to_string();
        if let Some(RefName::RemoteBranch { branch, remote: _ }) = parse_git_ref(&full_name) {
            // "refs/remotes/origin/HEAD" isn't a real remote-tracking branch
            if &branch == "HEAD" {
                continue;
            }
        }
        let git_commit = match git_ref.peel_to_commit() {
            Ok(git_commit) => git_commit,
            Err(_) => {
                // Perhaps a tag pointing to a GPG key or similar. Just skip it.
                continue;
            }
        };
        let id = CommitId::from_bytes(git_commit.id().as_bytes());
        new_git_heads.insert(id.clone());
        // TODO: Make it configurable which remotes are publishing and update public
        // heads here.
        mut_repo.set_git_ref(full_name.clone(), RefTarget::Normal(id.clone()));
        let old_target = existing_git_refs.remove(&full_name);
        let new_target = Some(RefTarget::Normal(id.clone()));
        if new_target != old_target {
            let commit = store.get_commit(&id).unwrap();
            mut_repo.add_head(&commit);
            changed_git_refs.insert(full_name, (old_target, new_target));
        }
    }
    for (full_name, target) in existing_git_refs {
        mut_repo.remove_git_ref(&full_name);
        changed_git_refs.insert(full_name, (Some(target), None));
    }
    for (full_name, (old_git_target, new_git_target)) in changed_git_refs {
        if let Some(ref_name) = parse_git_ref(&full_name) {
            // Apply the change that happened in git since last time we imported refs
            mut_repo.merge_single_ref(&ref_name, old_git_target.as_ref(), new_git_target.as_ref());
            // If a git remote-tracking branch changed, apply the change to the local branch
            // as well
            if let RefName::RemoteBranch { branch, remote: _ } = ref_name {
                mut_repo.merge_single_ref(
                    &RefName::LocalBranch(branch),
                    old_git_target.as_ref(),
                    new_git_target.as_ref(),
                );
            }
        }
    }

    // Find commits that are no longer referenced in the git repo and abandon them
    // in jj as well.
    let new_git_heads = new_git_heads.into_iter().collect_vec();
    // We could use mut_repo.record_rewrites() here but we know we only need to care
    // about abandoned commits for now. We may want to change this if we ever
    // add a way of preserving change IDs across rewrites by `git` (e.g. by
    // putting them in the commit message).
    let abandoned_commits = mut_repo
        .index()
        .walk_revs(&old_git_heads, &new_git_heads)
        .map(|entry| entry.commit_id())
        .collect_vec();
    let root_commit_id = mut_repo.store().root_commit_id().clone();
    for abandoned_commit in abandoned_commits {
        if abandoned_commit != root_commit_id {
            mut_repo.record_abandoned_commit(abandoned_commit);
        }
    }

    Ok(())
}

#[derive(Error, Debug, PartialEq)]
pub enum GitExportError {
    #[error("Cannot export conflicted branch '{0}'")]
    ConflictedBranch(String),
    #[error("Failed to read export state: {0}")]
    ReadStateError(String),
    #[error("Failed to write export state: {0}")]
    WriteStateError(String),
    #[error("Git error: {0}")]
    InternalGitError(#[from] git2::Error),
}

/// Reflect changes between two Jujutsu repo views in the underlying Git repo.
/// Returns a stripped-down repo view of the state we just exported, to be used
/// as `old_view` next time. Also returns a list of names of branches that
/// failed to export.
fn export_changes(
    mut_repo: &mut MutableRepo,
    old_view: &View,
    git_repo: &git2::Repository,
) -> Result<(op_store::View, Vec<String>), GitExportError> {
    let new_view = mut_repo.view();
    let old_branches: HashSet<_> = old_view.branches().keys().cloned().collect();
    let new_branches: HashSet<_> = new_view.branches().keys().cloned().collect();
    let mut branches_to_update = BTreeMap::new();
    let mut branches_to_delete = BTreeSet::new();
    // First find the changes we want need to make without modifying mut_repo
    for branch_name in old_branches.union(&new_branches) {
        let old_branch = old_view.get_local_branch(branch_name);
        let new_branch = new_view.get_local_branch(branch_name);
        if new_branch == old_branch {
            continue;
        }
        if let Some(new_branch) = new_branch {
            match new_branch {
                RefTarget::Normal(id) => {
                    branches_to_update
                        .insert(branch_name.clone(), Oid::from_bytes(id.as_bytes()).unwrap());
                }
                RefTarget::Conflict { .. } => {
                    // Skip conflicts and leave the old value in `exported_view`
                    continue;
                }
            }
        } else {
            branches_to_delete.insert(branch_name.clone());
        }
    }
    // TODO: Also check other worktrees' HEAD.
    if let Ok(head_ref) = git_repo.find_reference("HEAD") {
        if let (Some(head_git_ref), Ok(current_git_commit)) =
            (head_ref.symbolic_target(), head_ref.peel_to_commit())
        {
            if let Some(branch_name) = head_git_ref.strip_prefix("refs/heads/") {
                let detach_head = if let Some(new_target) = branches_to_update.get(branch_name) {
                    *new_target != current_git_commit.id()
                } else {
                    branches_to_delete.contains(branch_name)
                };
                if detach_head {
                    git_repo.set_head_detached(current_git_commit.id())?;
                }
            }
        }
    }
    let mut exported_view = old_view.store_view().clone();
    let mut failed_branches = vec![];
    for branch_name in branches_to_delete {
        let git_ref_name = format!("refs/heads/{}", branch_name);
        if let Ok(mut git_ref) = git_repo.find_reference(&git_ref_name) {
            if git_ref.delete().is_err() {
                failed_branches.push(branch_name);
                continue;
            }
        }
        exported_view.branches.remove(&branch_name);
        mut_repo.remove_git_ref(&git_ref_name);
    }
    for (branch_name, new_target) in branches_to_update {
        let git_ref_name = format!("refs/heads/{}", branch_name);
        if git_repo
            .reference(&git_ref_name, new_target, true, "export from jj")
            .is_err()
        {
            failed_branches.push(branch_name);
            continue;
        }
        exported_view.branches.insert(
            branch_name.clone(),
            BranchTarget {
                local_target: Some(RefTarget::Normal(CommitId::from_bytes(
                    new_target.as_bytes(),
                ))),
                remote_targets: Default::default(),
            },
        );
        mut_repo.set_git_ref(
            git_ref_name,
            RefTarget::Normal(CommitId::from_bytes(new_target.as_bytes())),
        );
    }
    Ok((exported_view, failed_branches))
}

/// Reflect changes made in the Jujutsu repo since last export in the underlying
/// Git repo. The exported view is recorded in the repo
/// (`.jj/repo/git_export_view`). Returns the names of any branches that failed
/// to export.
pub fn export_refs(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
) -> Result<Vec<String>, GitExportError> {
    upgrade_old_export_state(mut_repo.base_repo());

    let last_export_path = mut_repo.base_repo().repo_path().join("git_export_view");
    let last_export_store_view =
        if let Ok(mut last_export_file) = OpenOptions::new().read(true).open(&last_export_path) {
            let thrift_view = simple_op_store::read_thrift(&mut last_export_file)
                .map_err(|err| GitExportError::ReadStateError(err.to_string()))?;
            op_store::View::from(&thrift_view)
        } else {
            op_store::View::default()
        };
    let last_export_view = View::new(last_export_store_view);
    let (new_export_store_view, failed_branches) =
        export_changes(mut_repo, &last_export_view, git_repo)?;
    if let Ok(mut last_export_file) = OpenOptions::new()
        .write(true)
        .create(true)
        .open(&last_export_path)
    {
        let thrift_view = simple_op_store_model::View::from(&new_export_store_view);
        simple_op_store::write_thrift(&thrift_view, &mut last_export_file)
            .map_err(|err| GitExportError::WriteStateError(err.to_string()))?;
    }
    Ok(failed_branches)
}

fn upgrade_old_export_state(repo: &Arc<ReadonlyRepo>) {
    // Migrate repos that use the old git_export_operation_id file
    let last_operation_export_path = repo.repo_path().join("git_export_operation_id");
    if let Ok(mut last_operation_export_file) = OpenOptions::new()
        .read(true)
        .open(&last_operation_export_path)
    {
        let mut buf = vec![];
        last_operation_export_file.read_to_end(&mut buf).unwrap();
        let last_export_op_id = OperationId::from_hex(String::from_utf8(buf).unwrap().as_str());
        let loader = repo.loader();
        let op_store = loader.op_store();
        let last_export_store_op = op_store.read_operation(&last_export_op_id).unwrap();
        let last_export_store_view = op_store.read_view(&last_export_store_op.view_id).unwrap();
        if let Ok(mut last_export_file) = OpenOptions::new()
            .write(true)
            .create(true)
            .open(repo.repo_path().join("git_export_view"))
        {
            let thrift_view = simple_op_store_model::View::from(&last_export_store_view);
            simple_op_store::write_thrift(&thrift_view, &mut last_export_file)
                .map_err(|err| GitExportError::WriteStateError(err.to_string()))
                .unwrap();
        }
        std::fs::remove_file(last_operation_export_path).unwrap();
    }
}

#[derive(Error, Debug, PartialEq)]
pub enum GitFetchError {
    #[error("No git remote named '{0}'")]
    NoSuchRemote(String),
    // TODO: I'm sure there are other errors possible, such as transport-level errors.
    #[error("Unexpected git error when fetching: {0}")]
    InternalGitError(#[from] git2::Error),
}

#[tracing::instrument(skip(mut_repo, git_repo, callbacks))]
pub fn fetch(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
    remote_name: &str,
    callbacks: RemoteCallbacks<'_>,
) -> Result<Option<String>, GitFetchError> {
    let mut remote =
        git_repo
            .find_remote(remote_name)
            .map_err(|err| match (err.class(), err.code()) {
                (git2::ErrorClass::Config, git2::ErrorCode::NotFound) => {
                    GitFetchError::NoSuchRemote(remote_name.to_string())
                }
                (git2::ErrorClass::Config, git2::ErrorCode::InvalidSpec) => {
                    GitFetchError::NoSuchRemote(remote_name.to_string())
                }
                _ => GitFetchError::InternalGitError(err),
            })?;
    let mut fetch_options = git2::FetchOptions::new();
    let mut proxy_options = git2::ProxyOptions::new();
    proxy_options.auto();
    fetch_options.proxy_options(proxy_options);
    let callbacks = callbacks.into_git();
    fetch_options.remote_callbacks(callbacks);
    let refspec: &[&str] = &[];
    tracing::debug!("remote.download");
    remote.download(refspec, Some(&mut fetch_options))?;
    tracing::debug!("remote.update_tips");
    remote.update_tips(None, false, git2::AutotagOption::Unspecified, None)?;
    tracing::debug!("remote.prune");
    remote.prune(None)?;
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
    tracing::debug!("import_refs");
    import_refs(mut_repo, git_repo).map_err(|err| match err {
        GitImportError::InternalGitError(source) => GitFetchError::InternalGitError(source),
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

pub fn push_commit(
    git_repo: &git2::Repository,
    target: &Commit,
    remote_name: &str,
    remote_branch: &str,
    // TODO: We want this to be an Option<CommitId> for the expected current commit on the remote.
    // It's a blunt "force" option instead until git2-rs supports the "push negotiation" callback
    // (https://github.com/rust-lang/git2-rs/issues/733).
    force: bool,
    callbacks: RemoteCallbacks<'_>,
) -> Result<(), GitPushError> {
    push_updates(
        git_repo,
        remote_name,
        &[GitRefUpdate {
            qualified_name: format!("refs/heads/{}", remote_branch),
            force,
            new_target: Some(target.id().clone()),
        }],
        callbacks,
    )
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
    let mut remote =
        git_repo
            .find_remote(remote_name)
            .map_err(|err| match (err.class(), err.code()) {
                (git2::ErrorClass::Config, git2::ErrorCode::NotFound) => {
                    GitPushError::NoSuchRemote(remote_name.to_string())
                }
                (git2::ErrorClass::Config, git2::ErrorCode::InvalidSpec) => {
                    GitPushError::NoSuchRemote(remote_name.to_string())
                }
                _ => GitPushError::InternalGitError(err),
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
    pub get_ssh_key: Option<&'a mut dyn FnMut(&str) -> Option<PathBuf>>,
    pub get_password: Option<&'a mut dyn FnMut(&str, &str) -> Option<String>>,
    pub get_username_password: Option<&'a mut dyn FnMut(&str) -> Option<(String, String)>>,
}

impl<'a> RemoteCallbacks<'a> {
    fn into_git(mut self) -> git2::RemoteCallbacks<'a> {
        let mut callbacks = git2::RemoteCallbacks::new();
        if let Some(progress_cb) = self.progress {
            callbacks.transfer_progress(move |progress| {
                progress_cb(&Progress {
                    bytes_downloaded: if progress.received_objects() < progress.total_objects() {
                        Some(progress.received_bytes() as u64)
                    } else {
                        None
                    },
                    overall: (progress.indexed_objects() + progress.indexed_deltas()) as f32
                        / (progress.total_objects() + progress.total_deltas()) as f32,
                });
                true
            });
        }
        // TODO: We should expose the callbacks to the caller instead -- the library
        // crate shouldn't read environment variables.
        callbacks.credentials(move |url, username_from_url, allowed_types| {
            let span = tracing::debug_span!("RemoteCallbacks.credentials");
            let _ = span.enter();

            let git_config = git2::Config::open_default();
            let credential_helper = git_config
                .and_then(|conf| git2::Cred::credential_helper(&conf, url, username_from_url));
            if let Ok(creds) = credential_helper {
                tracing::debug!("using credential_helper");
                return Ok(creds);
            } else if let Some(username) = username_from_url {
                if allowed_types.contains(git2::CredentialType::SSH_KEY) {
                    if let Some((ssh_auth_sock, ssh_agent_pid)) = std::env::var("SSH_AUTH_SOCK")
                        .ok()
                        .zip(std::env::var("SSH_AGENT_PID").ok())
                    {
                        tracing::debug!(
                            username,
                            ssh_auth_sock,
                            ssh_agent_pid,
                            "using ssh_key_from_agent"
                        );
                        return git2::Cred::ssh_key_from_agent(username).map_err(|err| {
                            tracing::error!(err = %err);
                            err
                        });
                    }
                    if let Some(ref mut cb) = self.get_ssh_key {
                        if let Some(path) = cb(username) {
                            tracing::debug!(username, path = ?path, "using ssh_key");
                            return git2::Cred::ssh_key(username, None, &path, None).map_err(
                                |err| {
                                    tracing::error!(err = %err);
                                    err
                                },
                            );
                        }
                    }
                }
                if allowed_types.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
                    if let Some(ref mut cb) = self.get_password {
                        if let Some(pw) = cb(url, username) {
                            tracing::debug!(
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
                        tracing::debug!(username, "using userpass_plaintext");
                        return git2::Cred::userpass_plaintext(&username, &pw).map_err(|err| {
                            tracing::error!(err = %err);
                            err
                        });
                    }
                }
            }
            tracing::debug!("using default");
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
