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

#[derive(Debug, PartialEq, Eq)]
pub enum GitPushError {
    NotAGitRepo,
    NoSuchRemote,
    NotFastForward,
    // TODO: I'm sure there are other errors possible, such as transport-level errors,
    // and errors caused by the remote rejecting the push.
    InternalGitError(String),
}

pub fn push_commit(
    commit: &Commit,
    remote_name: &str,
    remote_branch: &str,
) -> Result<(), GitPushError> {
    let git_repo = commit.store().git_repo().ok_or(GitPushError::NotAGitRepo)?;
    let locked_git_repo = git_repo.lock().unwrap();
    // Create a temporary ref to work around https://github.com/libgit2/libgit2/issues/3178
    let temp_ref_name = format!("refs/jj/git-push/{}", commit.id().hex());
    let mut temp_ref = locked_git_repo
        .reference(
            &temp_ref_name,
            git2::Oid::from_bytes(&commit.id().0).unwrap(),
            true,
            "temporary reference for git push",
        )
        .map_err(|err| {
            GitPushError::InternalGitError(format!(
                "failed to create temporary git ref for push: {}",
                err
            ))
        })?;
    let mut remote = locked_git_repo.find_remote(remote_name).map_err(|err| {
        match (err.class(), err.code()) {
            (git2::ErrorClass::Config, git2::ErrorCode::NotFound) => GitPushError::NoSuchRemote,
            (git2::ErrorClass::Config, git2::ErrorCode::InvalidSpec) => GitPushError::NoSuchRemote,
            _ => panic!("unhandled git error: {:?}", err),
        }
    })?;
    // Need to add "refs/heads/" prefix due to https://github.com/libgit2/libgit2/issues/1125
    let refspec = format!("{}:refs/heads/{}", temp_ref_name, remote_branch);
    remote
        .push(&[refspec], None)
        .map_err(|err| match (err.class(), err.code()) {
            (git2::ErrorClass::Reference, git2::ErrorCode::NotFastForward) => {
                GitPushError::NotFastForward
            }
            _ => panic!("unhandled git error: {:?}", err),
        })?;
    temp_ref.delete().map_err(|err| {
        GitPushError::InternalGitError(format!(
            "failed to delete temporary git ref for push: {}",
            err
        ))
    })
}
