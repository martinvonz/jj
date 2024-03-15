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

//! Interfaces for indexes of the commits in a repository.

use std::any::Any;
use std::fmt::Debug;
use std::sync::Arc;

use thiserror::Error;

use crate::backend::{ChangeId, CommitId};
use crate::commit::Commit;
use crate::object_id::{HexPrefix, PrefixResolution};
use crate::op_store::OperationId;
use crate::operation::Operation;
use crate::revset::{ResolvedExpression, Revset, RevsetEvaluationError};
use crate::store::Store;

/// Returned if an error occurs while reading an index from the [`IndexStore`].
#[derive(Debug, Error)]
#[error(transparent)]
pub struct IndexReadError(pub Box<dyn std::error::Error + Send + Sync>);

/// Returned if an error occurs while writing an index to the [`IndexStore`].
#[derive(Debug, Error)]
#[error(transparent)]
pub struct IndexWriteError(pub Box<dyn std::error::Error + Send + Sync>);

/// An error returned if `Index::all_heads_for_gc()` is not supported by the
/// index backend.
#[derive(Debug, Error)]
#[error("Cannot collect all heads by index of this type")]
pub struct AllHeadsForGcUnsupported;

/// Defines the interface for types that provide persistent storage for an
/// index.
pub trait IndexStore: Send + Sync + Debug {
    #[allow(missing_docs)]
    fn as_any(&self) -> &dyn Any;

    /// Returns a name representing the type of index that the `IndexStore` is
    /// compatible with. For example, the `IndexStore` for the default index
    /// returns "default".
    fn name(&self) -> &str;

    /// Returns the index at the specified operation.
    fn get_index_at_op(
        &self,
        op: &Operation,
        store: &Arc<Store>,
    ) -> Result<Box<dyn ReadonlyIndex>, IndexReadError>;

    /// Writes `index` to the index store and returns a read-only version of the
    /// index.
    fn write_index(
        &self,
        index: Box<dyn MutableIndex>,
        op_id: &OperationId,
    ) -> Result<Box<dyn ReadonlyIndex>, IndexWriteError>;
}

/// Defines the interface for types that provide an index of the commits in a
/// repository by [`CommitId`].
pub trait Index: Send + Sync {
    /// Returns the minimum prefix length to disambiguate `commit_id` from other
    /// commits in the index. The length returned is the number of hexadecimal
    /// digits in the minimum prefix.
    ///
    /// If the given `commit_id` doesn't exist, returns the minimum prefix
    /// length which matches none of the commits in the index.
    fn shortest_unique_commit_id_prefix_len(&self, commit_id: &CommitId) -> usize;

    /// Searches the index for commit IDs matching `prefix`. Returns a
    /// [`PrefixResolution`] with a [`CommitId`] if the prefix matches a single
    /// commit.
    fn resolve_commit_id_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<CommitId>;

    /// Returns true if `commit_id` is present in the index.
    fn has_id(&self, commit_id: &CommitId) -> bool;

    /// Returns true if `ancestor_id` commit is an ancestor of the
    /// `descendant_id` commit, or if `ancestor_id` equals `descendant_id`.
    fn is_ancestor(&self, ancestor_id: &CommitId, descendant_id: &CommitId) -> bool;

    /// Returns the best common ancestor or ancestors of the commits in `set1`
    /// and `set2`. A "best common ancestor" has no descendants that are also
    /// common ancestors.
    fn common_ancestors(&self, set1: &[CommitId], set2: &[CommitId]) -> Vec<CommitId>;

    /// Heads among all indexed commits at the associated operation.
    ///
    /// Suppose the index contains all the historical heads and their
    /// ancestors/predecessors reachable from the associated operation, this
    /// function returns the heads that should be preserved on garbage
    /// collection.
    ///
    /// The iteration order is unspecified.
    fn all_heads_for_gc(
        &self,
    ) -> Result<Box<dyn Iterator<Item = CommitId> + '_>, AllHeadsForGcUnsupported>;

    /// Returns the subset of commit IDs in `candidates` which are not ancestors
    /// of other commits in `candidates`. If a commit id is duplicated in the
    /// `candidates` list it will appear at most once in the output.
    fn heads(&self, candidates: &mut dyn Iterator<Item = &CommitId>) -> Vec<CommitId>;

    /// Resolves the revset `expression` against the index and corresponding
    /// `store`.
    fn evaluate_revset<'index>(
        &'index self,
        expression: &ResolvedExpression,
        store: &Arc<Store>,
    ) -> Result<Box<dyn Revset + 'index>, RevsetEvaluationError>;
}

#[allow(missing_docs)]
pub trait ReadonlyIndex: Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn as_index(&self) -> &dyn Index;

    fn change_id_index(&self, heads: &mut dyn Iterator<Item = &CommitId>)
        -> Box<dyn ChangeIdIndex>;

    fn start_modification(&self) -> Box<dyn MutableIndex>;
}

#[allow(missing_docs)]
pub trait MutableIndex {
    fn as_any(&self) -> &dyn Any;

    fn into_any(self: Box<Self>) -> Box<dyn Any>;

    fn as_index(&self) -> &dyn Index;

    fn change_id_index(
        &self,
        heads: &mut dyn Iterator<Item = &CommitId>,
    ) -> Box<dyn ChangeIdIndex + '_>;

    fn add_commit(&mut self, commit: &Commit);

    fn merge_in(&mut self, other: &dyn ReadonlyIndex);
}

/// Defines the interface for types that provide an index of the commits in a
/// repository by [`ChangeId`].
pub trait ChangeIdIndex: Send + Sync {
    /// Resolve an unambiguous change ID prefix to the commit IDs in the index.
    ///
    /// The order of the returned commit IDs is unspecified.
    fn resolve_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<Vec<CommitId>>;

    /// This function returns the shortest length of a prefix of `key` that
    /// disambiguates it from every other key in the index.
    ///
    /// The length returned is a number of hexadecimal digits.
    ///
    /// This has some properties that we do not currently make much use of:
    ///
    /// - The algorithm works even if `key` itself is not in the index.
    ///
    /// - In the special case when there are keys in the trie for which our
    ///   `key` is an exact prefix, returns `key.len() + 1`. Conceptually, in
    ///   order to disambiguate, you need every letter of the key *and* the
    ///   additional fact that it's the entire key). This case is extremely
    ///   unlikely for hashes with 12+ hexadecimal characters.
    fn shortest_unique_prefix_len(&self, change_id: &ChangeId) -> usize;
}
