// Copyright 2023 The Jujutsu Authors
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

use crate::backend::{ChangeId, CommitId};
use crate::index::{HexPrefix, PrefixResolution};
use crate::repo::Repo;

pub struct IdPrefixContext<'repo> {
    repo: &'repo dyn Repo,
}

impl IdPrefixContext<'_> {
    pub fn new(repo: &dyn Repo) -> IdPrefixContext {
        IdPrefixContext { repo }
    }

    /// Resolve an unambiguous commit ID prefix.
    pub fn resolve_commit_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<CommitId> {
        self.repo.index().resolve_prefix(prefix)
    }

    /// Returns the shortest length of a prefix of `commit_id` that
    /// can still be resolved by `resolve_commit_prefix()`.
    pub fn shortest_commit_prefix_len(&self, commit_id: &CommitId) -> usize {
        self.repo
            .index()
            .shortest_unique_commit_id_prefix_len(commit_id)
    }

    /// Resolve an unambiguous change ID prefix to the commit IDs in the revset.
    pub fn resolve_change_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<Vec<CommitId>> {
        self.repo.resolve_change_id_prefix(prefix)
    }

    /// Returns the shortest length of a prefix of `change_id` that
    /// can still be resolved by `resolve_change_prefix()`.
    pub fn shortest_change_prefix_len(&self, change_id: &ChangeId) -> usize {
        self.repo.shortest_unique_change_id_prefix_len(change_id)
    }
}
