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

use crate::commit::Commit;
use crate::store::CommitId;
use crate::transaction::Transaction;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum GitImportError {
    #[error("The repo is not backed by a git repo")]
    NotAGitRepo,
    #[error("Unexpected git error when importing refs: {0}")]
    InternalGitError(#[from] git2::Error),
}

// Reflect changes made in the underlying Git repo in the Jujube repo.
pub fn import_refs(tx: &mut Transaction) -> Result<(), GitImportError> {
    let store = tx.store().clone();
    let git_repo = store.git_repo().ok_or(GitImportError::NotAGitRepo)?;
    let git_refs = git_repo.references()?;
    for git_ref in git_refs {
        let git_ref = git_ref?;
        if !(git_ref.is_tag() || git_ref.is_branch() || git_ref.is_remote()) {
            // Skip other refs (such as notes) and symbolic refs.
            // TODO: Is it useful to import HEAD (especially if it's detached)?
            continue;
        }
        let git_commit = git_ref.peel_to_commit()?;
        let id = CommitId(git_commit.id().as_bytes().to_vec());
        let commit = store.get_commit(&id).unwrap();
        tx.add_head(&commit);
    }
    Ok(())
}

#[derive(Error, Debug, PartialEq)]
pub enum GitFetchError {
    #[error("The repo is not backed by a git repo")]
    NotAGitRepo,
    #[error("No git remote named '{0}'")]
    NoSuchRemote(String),
    // TODO: I'm sure there are other errors possible, such as transport-level errors.
    #[error("Unexpected git error when fetching: {0}")]
    InternalGitError(#[from] git2::Error),
}

pub fn fetch(tx: &mut Transaction, remote_name: &str) -> Result<(), GitFetchError> {
    let git_repo = tx.store().git_repo().ok_or(GitFetchError::NotAGitRepo)?;
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
    let refspec: &[&str] = &[];
    remote.fetch(refspec, None, None)?;
    import_refs(tx).map_err(|err| match err {
        GitImportError::NotAGitRepo => panic!("git repo somehow became a non-git repo"),
        GitImportError::InternalGitError(source) => GitFetchError::InternalGitError(source),
    })?;
    Ok(())
}

#[derive(Error, Debug, PartialEq)]
pub enum GitPushError {
    #[error("The repo is not backed by a git repo")]
    NotAGitRepo,
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
    commit: &Commit,
    remote_name: &str,
    remote_branch: &str,
) -> Result<(), GitPushError> {
    let git_repo = commit.store().git_repo().ok_or(GitPushError::NotAGitRepo)?;
    // Create a temporary ref to work around https://github.com/libgit2/libgit2/issues/3178
    let temp_ref_name = format!("refs/jj/git-push/{}", commit.id().hex());
    let mut temp_ref = git_repo.reference(
        &temp_ref_name,
        git2::Oid::from_bytes(&commit.id().0).unwrap(),
        true,
        "temporary reference for git push",
    )?;
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
    // Need to add "refs/heads/" prefix due to https://github.com/libgit2/libgit2/issues/1125
    let qualified_remote_branch = format!("refs/heads/{}", remote_branch);
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
    let refspec = format!("{}:{}", temp_ref_name, qualified_remote_branch);
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
    temp_ref.delete()?;
    if updated {
        Ok(())
    } else {
        Err(GitPushError::RefUpdateRejected)
    }
}
