// Copyright 2020 Google LLC
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

use std::collections::{BTreeMap, HashSet};

use git2::RemoteCallbacks;
use itertools::Itertools;
use thiserror::Error;

use crate::backend::CommitId;
use crate::commit::Commit;
use crate::op_store::RefTarget;
use crate::repo::MutableRepo;
use crate::view::RefName;

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
            .split_once("/")
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

// Reflect changes made in the underlying Git repo in the Jujutsu repo.
pub fn import_refs(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
) -> Result<(), GitImportError> {
    let store = mut_repo.store().clone();
    let git_refs = git_repo.references()?;
    let mut existing_git_refs = mut_repo.view().git_refs().clone();
    let mut changed_git_refs = BTreeMap::new();
    for git_ref in git_refs {
        let git_ref = git_ref?;
        if !(git_ref.is_tag() || git_ref.is_branch() || git_ref.is_remote())
            || git_ref.name().is_none()
        {
            // Skip other refs (such as notes) and symbolic refs, as well as non-utf8 refs.
            // TODO: Is it useful to import HEAD (especially if it's detached)?
            continue;
        }
        let git_commit = match git_ref.peel_to_commit() {
            Ok(git_commit) => git_commit,
            Err(_) => {
                // Perhaps a tag pointing to a GPG key or similar. Just skip it.
                continue;
            }
        };
        let id = CommitId(git_commit.id().as_bytes().to_vec());
        let commit = store.get_commit(&id).unwrap();
        mut_repo.add_head(&commit);
        // TODO: Make it configurable which remotes are publishing and update public
        // heads here.
        let full_name = git_ref.name().unwrap().to_string();
        mut_repo.set_git_ref(full_name.clone(), RefTarget::Normal(id.clone()));
        let old_target = existing_git_refs.remove(&full_name);
        let new_target = Some(RefTarget::Normal(id));
        if new_target != old_target {
            changed_git_refs.insert(full_name, (old_target, new_target));
        }
    }
    for (full_name, target) in existing_git_refs {
        mut_repo.remove_git_ref(&full_name);
        // TODO: We should probably also remove heads pointing to the same
        // commits and commits no longer reachable from other refs.
        // If the underlying git repo has a branch that gets rewritten, we
        // should probably not keep the commits it used to point to.
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
    Ok(())
}

#[derive(Error, Debug, PartialEq)]
pub enum GitFetchError {
    #[error("No git remote named '{0}'")]
    NoSuchRemote(String),
    // TODO: I'm sure there are other errors possible, such as transport-level errors.
    #[error("Unexpected git error when fetching: {0}")]
    InternalGitError(#[from] git2::Error),
}

pub fn fetch(
    mut_repo: &mut MutableRepo,
    git_repo: &git2::Repository,
    remote_name: &str,
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
    let callbacks = create_remote_callbacks();
    fetch_options.remote_callbacks(callbacks);
    let refspec: &[&str] = &[];
    remote.download(refspec, Some(&mut fetch_options))?;
    remote.update_tips(None, false, git2::AutotagOption::Unspecified, None)?;
    remote.prune(None)?;
    // TODO: We could make it optional to get the default branch since we only care
    // about it on clone.
    let mut default_branch = None;
    if let Ok(default_ref_buf) = remote.default_branch() {
        if let Some(default_ref) = default_ref_buf.as_str() {
            // LocalBranch here is the local branch on the remote, so it's really the remote
            // branch
            if let Some(RefName::LocalBranch(branch_name)) = parse_git_ref(default_ref) {
                default_branch = Some(branch_name);
            }
        }
    }
    remote.disconnect()?;
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
    #[error("Remote reject the update of some refs")]
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
) -> Result<(), GitPushError> {
    push_updates(
        git_repo,
        remote_name,
        &[GitRefUpdate {
            qualified_name: format!("refs/heads/{}", remote_branch),
            force,
            new_target: Some(target.id().clone()),
        }],
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
                git2::Oid::from_bytes(&new_target.0).unwrap(),
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
    let result = push_refs(git_repo, remote_name, &qualified_remote_refs, &refspecs);
    for mut temp_ref in temp_refs {
        // TODO: Figure out how to do the equivalent of absl::Cleanup for
        // temp_ref.delete().
        temp_ref.delete()?;
    }
    result
}

fn push_refs(
    git_repo: &git2::Repository,
    remote_name: &str,
    qualified_remote_refs: &[&str],
    refspecs: &[String],
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
    let mut callbacks = create_remote_callbacks();
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

fn create_remote_callbacks() -> RemoteCallbacks<'static> {
    let mut callbacks = git2::RemoteCallbacks::new();
    // TODO: We should expose the callbacks to the caller instead -- the library
    // crate shouldn't look in $HOME etc.
    callbacks.credentials(|_url, username_from_url, allowed_types| {
        if allowed_types.contains(git2::CredentialType::SSH_KEY) {
            if std::env::var("SSH_AGENT_PID").is_ok() {
                return git2::Cred::ssh_key_from_agent(username_from_url.unwrap());
            }
            if let Ok(home_dir) = std::env::var("HOME") {
                let key_path = std::path::Path::new(&home_dir).join(".ssh").join("id_rsa");
                if key_path.is_file() {
                    return git2::Cred::ssh_key(username_from_url.unwrap(), None, &key_path, None);
                }
            }
        }
        git2::Cred::default()
    });
    callbacks
}
