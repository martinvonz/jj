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

use std::collections::BTreeMap;

use thiserror::Error;

use crate::commit::Commit;
use crate::op_store::RefTarget;
use crate::repo::MutableRepo;
use crate::store::CommitId;
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
        // For now, we consider all remotes "publishing".
        // TODO: Make it configurable which remotes are publishing.
        if git_ref.is_remote() {
            mut_repo.add_public_head(&commit);
        }
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
) -> Result<(), GitFetchError> {
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
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.credentials(|_url, username_from_url, _allowed_types| {
        git2::Cred::ssh_key_from_agent(username_from_url.unwrap())
    });
    let mut fetch_options = git2::FetchOptions::new();
    fetch_options.remote_callbacks(callbacks);
    let refspec: &[&str] = &[];
    remote.fetch(refspec, Some(&mut fetch_options), None)?;
    import_refs(mut_repo, git_repo).map_err(|err| match err {
        GitImportError::InternalGitError(source) => GitFetchError::InternalGitError(source),
    })?;
    Ok(())
}

#[derive(Error, Debug, PartialEq)]
pub enum GitPushError {
    #[error("No git remote named '{0}'")]
    NoSuchRemote(String),
    #[error("Push is not fast-forwardable'")]
    NotFastForward,
    #[error("Remote reject the update'")]
    RefUpdateRejected,
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
) -> Result<(), GitPushError> {
    // Create a temporary ref to work around https://github.com/libgit2/libgit2/issues/3178
    let temp_ref_name = format!("refs/jj/git-push/{}", target.id().hex());
    let mut temp_ref = git_repo.reference(
        &temp_ref_name,
        git2::Oid::from_bytes(&target.id().0).unwrap(),
        true,
        "temporary reference for git push",
    )?;
    // Need to add "refs/heads/" prefix due to https://github.com/libgit2/libgit2/issues/1125
    let qualified_remote_branch = format!("refs/heads/{}", remote_branch);
    let refspec = format!("{}:{}", temp_ref_name, qualified_remote_branch);
    let result = push_ref(git_repo, remote_name, &qualified_remote_branch, &refspec);
    // TODO: Figure out how to do the equivalent of absl::Cleanup for
    // temp_ref.delete().
    temp_ref.delete()?;
    result
}

fn push_ref(
    git_repo: &git2::Repository,
    remote_name: &str,
    qualified_remote_branch: &str,
    refspec: &str,
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
    let mut callbacks = git2::RemoteCallbacks::new();
    let mut updated = false;
    callbacks.credentials(|_url, username_from_url, _allowed_types| {
        git2::Cred::ssh_key_from_agent(username_from_url.unwrap())
    });
    callbacks.push_update_reference(|refname, status| {
        if refname == qualified_remote_branch && status.is_none() {
            updated = true;
        }
        Ok(())
    });
    let mut push_options = git2::PushOptions::new();
    push_options.remote_callbacks(callbacks);
    remote
        .push(&[refspec], Some(&mut push_options))
        .map_err(|err| match (err.class(), err.code()) {
            (git2::ErrorClass::Reference, git2::ErrorCode::NotFastForward) => {
                GitPushError::NotFastForward
            }
            _ => GitPushError::InternalGitError(err),
        })?;
    drop(push_options);
    if updated {
        Ok(())
    } else {
        Err(GitPushError::RefUpdateRejected)
    }
}
