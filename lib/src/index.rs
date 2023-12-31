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

use crate::backend::{CommitId, ObjectId};
use crate::commit::Commit;
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

    fn evaluate_revset<'index>(
        &'index self,
        expression: &ResolvedExpression,
        store: &Arc<Store>,
    ) -> Result<Box<dyn Revset<'index> + 'index>, RevsetEvaluationError>;
}

pub trait ReadonlyIndex: Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn as_index(&self) -> &dyn Index;

    // TODO: might be better to split Index::evaluate_revset() to
    // Readonly/MutableIndex::evaluate_static().
    fn evaluate_revset_static(
        &self,
        expression: &ResolvedExpression,
        store: &Arc<Store>,
    ) -> Result<Box<dyn Revset<'static>>, RevsetEvaluationError>;

    fn start_modification(&self) -> Box<dyn MutableIndex>;
}

pub trait MutableIndex {
    fn as_any(&self) -> &dyn Any;

    fn into_any(self: Box<Self>) -> Box<dyn Any>;

    fn as_index(&self) -> &dyn Index;

    fn add_commit(&mut self, commit: &Commit);

    fn merge_in(&mut self, other: &dyn ReadonlyIndex);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexPrefix {
    // For odd-length prefix, lower 4 bits of the last byte is padded with 0
    min_prefix_bytes: Vec<u8>,
    has_odd_byte: bool,
}

impl HexPrefix {
    pub fn new(prefix: &str) -> Option<HexPrefix> {
        let has_odd_byte = prefix.len() & 1 != 0;
        let min_prefix_bytes = if has_odd_byte {
            hex::decode(prefix.to_owned() + "0").ok()?
        } else {
            hex::decode(prefix).ok()?
        };
        Some(HexPrefix {
            min_prefix_bytes,
            has_odd_byte,
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        HexPrefix {
            min_prefix_bytes: bytes.to_owned(),
            has_odd_byte: false,
        }
    }

    pub fn hex(&self) -> String {
        let mut hex_string = hex::encode(&self.min_prefix_bytes);
        if self.has_odd_byte {
            hex_string.pop().unwrap();
        }
        hex_string
    }

    /// Minimum bytes that would match this prefix. (e.g. "abc0" for "abc")
    ///
    /// Use this to partition a sorted slice, and test `matches(id)` from there.
    pub fn min_prefix_bytes(&self) -> &[u8] {
        &self.min_prefix_bytes
    }

    /// Returns the bytes representation if this prefix can be a full id.
    pub fn as_full_bytes(&self) -> Option<&[u8]> {
        (!self.has_odd_byte).then_some(&self.min_prefix_bytes)
    }

    fn split_odd_byte(&self) -> (Option<u8>, &[u8]) {
        if self.has_odd_byte {
            let (&odd, prefix) = self.min_prefix_bytes.split_last().unwrap();
            (Some(odd), prefix)
        } else {
            (None, &self.min_prefix_bytes)
        }
    }

    pub fn matches<Q: ObjectId>(&self, id: &Q) -> bool {
        let id_bytes = id.as_bytes();
        let (maybe_odd, prefix) = self.split_odd_byte();
        if id_bytes.starts_with(prefix) {
            if let Some(odd) = maybe_odd {
                matches!(id_bytes.get(prefix.len()), Some(v) if v & 0xf0 == odd)
            } else {
                true
            }
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixResolution<T> {
    NoMatch,
    SingleMatch(T),
    AmbiguousMatch,
}

impl<T> PrefixResolution<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> PrefixResolution<U> {
        match self {
            PrefixResolution::NoMatch => PrefixResolution::NoMatch,
            PrefixResolution::SingleMatch(x) => PrefixResolution::SingleMatch(f(x)),
            PrefixResolution::AmbiguousMatch => PrefixResolution::AmbiguousMatch,
        }
    }
}

impl<T: Clone> PrefixResolution<T> {
    pub fn plus(&self, other: &PrefixResolution<T>) -> PrefixResolution<T> {
        match (self, other) {
            (PrefixResolution::NoMatch, other) => other.clone(),
            (local, PrefixResolution::NoMatch) => local.clone(),
            (PrefixResolution::AmbiguousMatch, _) => PrefixResolution::AmbiguousMatch,
            (_, PrefixResolution::AmbiguousMatch) => PrefixResolution::AmbiguousMatch,
            (PrefixResolution::SingleMatch(_), PrefixResolution::SingleMatch(_)) => {
                PrefixResolution::AmbiguousMatch
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_prefix_prefixes() {
        let prefix = HexPrefix::new("").unwrap();
        assert_eq!(prefix.min_prefix_bytes(), b"");

        let prefix = HexPrefix::new("1").unwrap();
        assert_eq!(prefix.min_prefix_bytes(), b"\x10");

        let prefix = HexPrefix::new("12").unwrap();
        assert_eq!(prefix.min_prefix_bytes(), b"\x12");

        let prefix = HexPrefix::new("123").unwrap();
        assert_eq!(prefix.min_prefix_bytes(), b"\x12\x30");
    }

    #[test]
    fn test_hex_prefix_matches() {
        let id = CommitId::from_hex("1234");

        assert!(HexPrefix::new("").unwrap().matches(&id));
        assert!(HexPrefix::new("1").unwrap().matches(&id));
        assert!(HexPrefix::new("12").unwrap().matches(&id));
        assert!(HexPrefix::new("123").unwrap().matches(&id));
        assert!(HexPrefix::new("1234").unwrap().matches(&id));
        assert!(!HexPrefix::new("12345").unwrap().matches(&id));

        assert!(!HexPrefix::new("a").unwrap().matches(&id));
        assert!(!HexPrefix::new("1a").unwrap().matches(&id));
        assert!(!HexPrefix::new("12a").unwrap().matches(&id));
        assert!(!HexPrefix::new("123a").unwrap().matches(&id));
    }
}
