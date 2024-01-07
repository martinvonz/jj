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

/// Error while reading index from the `IndexStore`.
#[derive(Debug, Error)]
#[error(transparent)]
pub struct IndexReadError(pub Box<dyn std::error::Error + Send + Sync>);

/// Error while writing index to the `IndexStore`.
#[derive(Debug, Error)]
#[error(transparent)]
pub struct IndexWriteError(pub Box<dyn std::error::Error + Send + Sync>);

pub trait IndexStore: Send + Sync + Debug {
    fn as_any(&self) -> &dyn Any;

    fn name(&self) -> &str;

    fn get_index_at_op(
        &self,
        op: &Operation,
        store: &Arc<Store>,
    ) -> Result<Box<dyn ReadonlyIndex>, IndexReadError>;

    fn write_index(
        &self,
        index: Box<dyn MutableIndex>,
        op_id: &OperationId,
    ) -> Result<Box<dyn ReadonlyIndex>, IndexWriteError>;
}

pub trait Index: Send + Sync {
    fn shortest_unique_commit_id_prefix_len(&self, commit_id: &CommitId) -> usize;

    fn resolve_commit_id_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<CommitId>;

    fn has_id(&self, commit_id: &CommitId) -> bool;

    fn is_ancestor(&self, ancestor_id: &CommitId, descendant_id: &CommitId) -> bool;

    fn common_ancestors(&self, set1: &[CommitId], set2: &[CommitId]) -> Vec<CommitId>;

    fn heads(&self, candidates: &mut dyn Iterator<Item = &CommitId>) -> Vec<CommitId>;

    /// Parents before children
    fn topo_order(&self, input: &mut dyn Iterator<Item = &CommitId>) -> Vec<CommitId>;

    fn change_id_index(
        &self,
        heads: &mut dyn Iterator<Item = &CommitId>,
    ) -> Box<dyn ChangeIdIndex + '_>;

    fn evaluate_revset<'index>(
        &'index self,
        expression: &ResolvedExpression,
        store: &Arc<Store>,
    ) -> Result<Box<dyn Revset<'index> + 'index>, RevsetEvaluationError>;
}

pub trait ReadonlyIndex: Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn as_index(&self) -> &dyn Index;

    // TODO: might be better to split Index::change_id_index() to
    // Readonly/MutableIndex::change_id_index_static().
    fn change_id_index_static(
        &self,
        heads: &mut dyn Iterator<Item = &CommitId>,
    ) -> Box<dyn ChangeIdIndex>;

    fn start_modification(&self) -> Box<dyn MutableIndex>;
}

pub trait MutableIndex {
    fn as_any(&self) -> &dyn Any;

    fn into_any(self: Box<Self>) -> Box<dyn Any>;

    fn as_index(&self) -> &dyn Index;

    fn add_commit(&mut self, commit: &Commit);

    fn merge_in(&mut self, other: &dyn ReadonlyIndex);
}

pub trait ChangeIdIndex: Send + Sync {
    /// Resolve an unambiguous change ID prefix to the commit IDs in the index.
    fn resolve_prefix(&self, prefix: &HexPrefix) -> PrefixResolution<Vec<CommitId>>;

    /// This function returns the shortest length of a prefix of `key` that
    /// disambiguates it from every other key in the index.
    ///
    /// The length to be returned is a number of hexadecimal digits.
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
