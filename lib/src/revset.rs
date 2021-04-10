// Copyright 2021 Google LLC
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

use thiserror::Error;

use crate::commit::Commit;
use crate::index::{HexPrefix, PrefixResolution};
use crate::repo::RepoRef;
use crate::store::{CommitId, StoreError};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RevsetError {
    #[error("Revision \"{0}\" doesn't exist")]
    NoSuchRevision(String),
    #[error("Commit id prefix \"{0}\" is ambiguous")]
    AmbiguousCommitIdPrefix(String),
    #[error("Unexpected error from store: {0}")]
    StoreError(#[from] StoreError),
}

// TODO: Decide if we should allow a single symbol to resolve to multiple
// revisions. For example, we may want to resolve a change id to all the
// matching commits. Depending on how we decide to handle divergent git refs and
// similar, we may also want those to resolve to multiple commits.
pub fn resolve_symbol(repo: RepoRef, symbol: &str) -> Result<Commit, RevsetError> {
    // TODO: Support change ids.
    if symbol == "@" {
        Ok(repo.store().get_commit(repo.view().checkout())?)
    } else if symbol == "root" {
        Ok(repo.store().root_commit())
    } else {
        // Try to resolve as a git ref
        let view = repo.view();
        for git_ref_prefix in &["", "refs/", "refs/heads/", "refs/tags/", "refs/remotes/"] {
            if let Some(commit_id) = view.git_refs().get(&(git_ref_prefix.to_string() + symbol)) {
                return Ok(repo.store().get_commit(&commit_id)?);
            }
        }

        // Try to resolve as a commit id. First check if it's a full commit id.
        if let Ok(binary_commit_id) = hex::decode(symbol) {
            let commit_id = CommitId(binary_commit_id);
            match repo.store().get_commit(&commit_id) {
                Ok(commit) => return Ok(commit),
                Err(StoreError::NotFound) => {} // fall through
                Err(err) => return Err(RevsetError::StoreError(err)),
            }
        }

        if let Some(prefix) = HexPrefix::new(symbol.to_string()) {
            match repo.index().resolve_prefix(&prefix) {
                PrefixResolution::NoMatch => {
                    return Err(RevsetError::NoSuchRevision(symbol.to_owned()))
                }
                PrefixResolution::AmbiguousMatch => {
                    return Err(RevsetError::AmbiguousCommitIdPrefix(symbol.to_owned()))
                }
                PrefixResolution::SingleMatch(commit_id) => {
                    return Ok(repo.store().get_commit(&commit_id)?)
                }
            }
        }

        Err(RevsetError::NoSuchRevision(symbol.to_owned()))
    }
}
