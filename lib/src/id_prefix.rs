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

#![allow(missing_docs)]

use std::iter;
use std::marker::PhantomData;
use std::rc::Rc;

use itertools::Itertools as _;
use once_cell::unsync::OnceCell;

use crate::backend::{ChangeId, CommitId};
use crate::hex_util;
use crate::object_id::{HexPrefix, ObjectId, PrefixResolution};
use crate::repo::Repo;
use crate::revset::{DefaultSymbolResolver, RevsetExpression};

struct PrefixDisambiguationError;

struct DisambiguationData {
    expression: Rc<RevsetExpression>,
    indexes: OnceCell<Indexes>,
}

struct Indexes {
    commit_change_ids: Vec<(CommitId, ChangeId)>,
    commit_index: IdIndex<CommitId, u32, 4>,
    change_index: IdIndex<ChangeId, u32, 4>,
}

impl DisambiguationData {
    fn indexes(&self, repo: &dyn Repo) -> Result<&Indexes, PrefixDisambiguationError> {
        self.indexes.get_or_try_init(|| {
            let symbol_resolver = DefaultSymbolResolver::new(repo);
            let resolved_expression = self
                .expression
                .clone()
                .resolve_user_expression(repo, &symbol_resolver)
                .map_err(|_| PrefixDisambiguationError)?;
            let revset = resolved_expression
                .evaluate(repo)
                .map_err(|_| PrefixDisambiguationError)?;

            let commit_change_ids = revset.commit_change_ids().collect_vec();
            let mut commit_index = IdIndex::with_capacity(commit_change_ids.len());
            let mut change_index = IdIndex::with_capacity(commit_change_ids.len());
            for (i, (commit_id, change_id)) in commit_change_ids.iter().enumerate() {
                let i: u32 = i.try_into().unwrap();
                commit_index.insert(commit_id, i);
                change_index.insert(change_id, i);
            }
            Ok(Indexes {
                commit_change_ids,
                commit_index: commit_index.build(),
                change_index: change_index.build(),
            })
        })
    }
}

impl<'a> IdIndexSource<u32> for &'a [(CommitId, ChangeId)] {
    type Entry = &'a (CommitId, ChangeId);

    fn entry_at(&self, pointer: &u32) -> Self::Entry {
        &self[*pointer as usize]
    }
}

impl IdIndexSourceEntry<CommitId> for &'_ (CommitId, ChangeId) {
    fn to_key(&self) -> CommitId {
        let (commit_id, _) = self;
        commit_id.clone()
    }
}

impl IdIndexSourceEntry<ChangeId> for &'_ (CommitId, ChangeId) {
    fn to_key(&self) -> ChangeId {
        let (_, change_id) = self;
        change_id.clone()
    }
}

#[derive(Default)]
pub struct IdPrefixContext {
    disambiguation: Option<DisambiguationData>,
}

impl IdPrefixContext {
    pub fn disambiguate_within(mut self, expression: Rc<RevsetExpression>) -> Self {
        self.disambiguation = Some(DisambiguationData {
            expression,
            indexes: OnceCell::new(),
        });
        self
    }

    fn disambiguation_indexes(&self, repo: &dyn Repo) -> Option<&Indexes> {
        // TODO: propagate errors instead of treating them as if no revset was specified
        self.disambiguation
            .as_ref()
            .and_then(|disambiguation| disambiguation.indexes(repo).ok())
    }

    /// Resolve an unambiguous commit ID prefix.
    pub fn resolve_commit_prefix(
        &self,
        repo: &dyn Repo,
        prefix: &HexPrefix,
    ) -> PrefixResolution<CommitId> {
        if let Some(indexes) = self.disambiguation_indexes(repo) {
            let resolution = indexes
                .commit_index
                .resolve_prefix_to_key(&*indexes.commit_change_ids, prefix);
            if let PrefixResolution::SingleMatch(id) = resolution {
                return PrefixResolution::SingleMatch(id);
            }
        }
        repo.index().resolve_commit_id_prefix(prefix)
    }

    /// Returns the shortest length of a prefix of `commit_id` that
    /// can still be resolved by `resolve_commit_prefix()`.
    pub fn shortest_commit_prefix_len(&self, repo: &dyn Repo, commit_id: &CommitId) -> usize {
        if let Some(indexes) = self.disambiguation_indexes(repo) {
            if let Some(lookup) = indexes
                .commit_index
                .lookup_exact(&*indexes.commit_change_ids, commit_id)
            {
                return lookup.shortest_unique_prefix_len();
            }
        }
        repo.index().shortest_unique_commit_id_prefix_len(commit_id)
    }

    /// Resolve an unambiguous change ID prefix to the commit IDs in the revset.
    pub fn resolve_change_prefix(
        &self,
        repo: &dyn Repo,
        prefix: &HexPrefix,
    ) -> PrefixResolution<Vec<CommitId>> {
        if let Some(indexes) = self.disambiguation_indexes(repo) {
            let resolution = indexes.change_index.resolve_prefix_with(
                &*indexes.commit_change_ids,
                prefix,
                |(commit_id, _)| commit_id.clone(),
            );
            if let PrefixResolution::SingleMatch((_, ids)) = resolution {
                return PrefixResolution::SingleMatch(ids);
            }
        }
        repo.resolve_change_id_prefix(prefix)
    }

    /// Returns the shortest length of a prefix of `change_id` that
    /// can still be resolved by `resolve_change_prefix()`.
    pub fn shortest_change_prefix_len(&self, repo: &dyn Repo, change_id: &ChangeId) -> usize {
        if let Some(indexes) = self.disambiguation_indexes(repo) {
            if let Some(lookup) = indexes
                .change_index
                .lookup_exact(&*indexes.commit_change_ids, change_id)
            {
                return lookup.shortest_unique_prefix_len();
            }
        }
        repo.shortest_unique_change_id_prefix_len(change_id)
    }
}

/// In-memory immutable index to do prefix lookup of key `K` through `P`.
///
/// In a nutshell, this is a mapping of `K` -> `P` -> `S::Entry` where `S:
/// IdIndexSource<P>`. The source table `S` isn't owned by this index.
///
/// This index stores first `N` bytes of each key `K` associated with the
/// pointer `P`. `K` may be a heap-allocated object. `P` is supposed to be
/// a cheap value type like `u32` or `usize`. As the index entry of type
/// `([u8; N], P)` is small and has no indirect reference, constructing
/// the index should be faster than sorting the source `(K, _)` pairs.
///
/// A key `K` must be at least `N` bytes long.
#[derive(Clone, Debug)]
pub struct IdIndex<K, P, const N: usize> {
    // Maybe better to build separate (keys, values) vectors, but there's no std function
    // to co-sort them.
    index: Vec<([u8; N], P)>,
    // Let's pretend [u8; N] above were of type K. It helps type inference, and ensures that
    // IdIndexSource has the same key type.
    phantom_key: PhantomData<K>,
}

/// Source table for `IdIndex` to map pointer of type `P` to entry.
pub trait IdIndexSource<P> {
    type Entry;

    fn entry_at(&self, pointer: &P) -> Self::Entry;
}

/// Source table entry of `IdIndex`, which is conceptually a `(key, value)`
/// pair.
pub trait IdIndexSourceEntry<K> {
    fn to_key(&self) -> K;
}

#[derive(Clone, Debug)]
pub struct IdIndexBuilder<K, P, const N: usize> {
    unsorted_index: Vec<([u8; N], P)>,
    phantom_key: PhantomData<K>,
}

impl<K, P, const N: usize> IdIndexBuilder<K, P, N>
where
    K: ObjectId + Ord,
{
    /// Inserts new entry. Multiple values can be associated with a single key.
    pub fn insert(&mut self, key: &K, pointer: P) {
        let short_key = unwrap_as_short_key(key.as_bytes());
        self.unsorted_index.push((*short_key, pointer));
    }

    pub fn build(self) -> IdIndex<K, P, N> {
        let mut index = self.unsorted_index;
        index.sort_unstable_by_key(|(s, _)| *s);
        let phantom_key = self.phantom_key;
        IdIndex { index, phantom_key }
    }
}

impl<K, P, const N: usize> IdIndex<K, P, N>
where
    K: ObjectId + Ord,
{
    pub fn builder() -> IdIndexBuilder<K, P, N> {
        IdIndexBuilder {
            unsorted_index: Vec::new(),
            phantom_key: PhantomData,
        }
    }

    pub fn with_capacity(capacity: usize) -> IdIndexBuilder<K, P, N> {
        IdIndexBuilder {
            unsorted_index: Vec::with_capacity(capacity),
            phantom_key: PhantomData,
        }
    }

    /// Looks up entries with the given prefix, and collects values if matched
    /// entries have unambiguous keys.
    pub fn resolve_prefix_with<B, S, U>(
        &self,
        source: S,
        prefix: &HexPrefix,
        entry_mapper: impl FnMut(S::Entry) -> U,
    ) -> PrefixResolution<(K, B)>
    where
        B: FromIterator<U>,
        S: IdIndexSource<P>,
        S::Entry: IdIndexSourceEntry<K>,
    {
        fn collect<B, K, E, U>(
            mut range: impl Iterator<Item = (K, E)>,
            mut entry_mapper: impl FnMut(E) -> U,
        ) -> PrefixResolution<(K, B)>
        where
            B: FromIterator<U>,
            K: Eq,
        {
            if let Some((first_key, first_entry)) = range.next() {
                let maybe_values: Option<B> = iter::once(Some(entry_mapper(first_entry)))
                    .chain(range.map(|(k, e)| (k == first_key).then(|| entry_mapper(e))))
                    .collect();
                if let Some(values) = maybe_values {
                    PrefixResolution::SingleMatch((first_key, values))
                } else {
                    PrefixResolution::AmbiguousMatch
                }
            } else {
                PrefixResolution::NoMatch
            }
        }

        let min_bytes = prefix.min_prefix_bytes();
        if min_bytes.is_empty() {
            // We consider an empty prefix ambiguous even if the index has a single entry.
            return PrefixResolution::AmbiguousMatch;
        }

        let to_key_entry_pair = |(_, pointer): &(_, P)| -> (K, S::Entry) {
            let entry = source.entry_at(pointer);
            (entry.to_key(), entry)
        };
        if min_bytes.len() > N {
            // If the min prefix (including odd byte) is longer than the stored short keys,
            // we are sure that min_bytes[..N] does not include the odd byte. Use it to
            // take contiguous range, then filter by (longer) prefix.matches().
            let short_bytes = unwrap_as_short_key(min_bytes);
            let pos = self.index.partition_point(|(s, _)| s < short_bytes);
            let range = self.index[pos..]
                .iter()
                .take_while(|(s, _)| s == short_bytes)
                .map(to_key_entry_pair)
                .filter(|(k, _)| prefix.matches(k));
            collect(range, entry_mapper)
        } else {
            // Otherwise, use prefix.matches() to deal with odd byte. Since the prefix is
            // covered by short key width, we're sure that the matching prefixes are sorted.
            let pos = self.index.partition_point(|(s, _)| &s[..] < min_bytes);
            let range = self.index[pos..]
                .iter()
                .map(to_key_entry_pair)
                .take_while(|(k, _)| prefix.matches(k));
            collect(range, entry_mapper)
        }
    }

    /// Looks up unambiguous key with the given prefix.
    pub fn resolve_prefix_to_key<S>(&self, source: S, prefix: &HexPrefix) -> PrefixResolution<K>
    where
        S: IdIndexSource<P>,
        S::Entry: IdIndexSourceEntry<K>,
    {
        self.resolve_prefix_with(source, prefix, |_| ())
            .map(|(key, ())| key)
    }

    /// Looks up entry for the key. Returns accessor to neighbors.
    pub fn lookup_exact<'i, 'q, S>(
        &'i self,
        source: S,
        key: &'q K,
    ) -> Option<IdIndexLookup<'i, 'q, K, P, S, N>>
    where
        S: IdIndexSource<P>,
        S::Entry: IdIndexSourceEntry<K>,
    {
        let lookup = self.lookup_some(source, key);
        lookup.has_key().then_some(lookup)
    }

    fn lookup_some<'i, 'q, S>(&'i self, source: S, key: &'q K) -> IdIndexLookup<'i, 'q, K, P, S, N>
    where
        S: IdIndexSource<P>,
    {
        let short_key = unwrap_as_short_key(key.as_bytes());
        let index = &self.index;
        let pos = index.partition_point(|(s, _)| s < short_key);
        IdIndexLookup {
            index,
            source,
            key,
            pos,
        }
    }

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
    pub fn shortest_unique_prefix_len<S>(&self, source: S, key: &K) -> usize
    where
        S: IdIndexSource<P>,
        S::Entry: IdIndexSourceEntry<K>,
    {
        self.lookup_some(source, key).shortest_unique_prefix_len()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct IdIndexLookup<'i, 'q, K, P, S, const N: usize> {
    index: &'i Vec<([u8; N], P)>,
    source: S,
    key: &'q K,
    pos: usize, // may be index.len()
}

impl<'i, 'q, K, P, S, const N: usize> IdIndexLookup<'i, 'q, K, P, S, N>
where
    K: ObjectId + Eq,
    S: IdIndexSource<P>,
    S::Entry: IdIndexSourceEntry<K>,
{
    fn has_key(&self) -> bool {
        let short_key = unwrap_as_short_key(self.key.as_bytes());
        self.index[self.pos..]
            .iter()
            .take_while(|(s, _)| s == short_key)
            .any(|(_, p)| self.source.entry_at(p).to_key() == *self.key)
    }

    pub fn shortest_unique_prefix_len(&self) -> usize {
        // Since entries having the same short key aren't sorted by the full-length key,
        // we need to scan all entries in the current chunk, plus left/right neighbors.
        // Typically, current.len() is 1.
        let short_key = unwrap_as_short_key(self.key.as_bytes());
        let left = self.pos.checked_sub(1).map(|p| &self.index[p]);
        let (current, right) = {
            let range = &self.index[self.pos..];
            let count = range.iter().take_while(|(s, _)| s == short_key).count();
            (&range[..count], range.get(count))
        };

        // Left/right neighbors should have unique short keys. For the current chunk,
        // we need to look up full-length keys.
        let unique_len = |a: &[u8], b: &[u8]| hex_util::common_hex_len(a, b) + 1;
        let neighbor_lens = left
            .iter()
            .chain(&right)
            .map(|(s, _)| unique_len(s, short_key));
        let current_lens = current
            .iter()
            .map(|(_, p)| self.source.entry_at(p).to_key())
            .filter(|key| key != self.key)
            .map(|key| unique_len(key.as_bytes(), self.key.as_bytes()));
        // Even if the key is the only one in the index, we require at least one digit.
        neighbor_lens.chain(current_lens).max().unwrap_or(1)
    }
}

fn unwrap_as_short_key<const N: usize>(key_bytes: &[u8]) -> &[u8; N] {
    let short_slice = key_bytes.get(..N).expect("key too short");
    short_slice.try_into().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Eq, PartialEq)]
    struct Position(usize);

    impl<'a, V> IdIndexSource<Position> for &'a [(ChangeId, V)] {
        type Entry = &'a (ChangeId, V);

        fn entry_at(&self, pointer: &Position) -> Self::Entry {
            &self[pointer.0]
        }
    }

    impl<V> IdIndexSourceEntry<ChangeId> for &'_ (ChangeId, V) {
        fn to_key(&self) -> ChangeId {
            let (change_id, _) = self;
            change_id.clone()
        }
    }

    fn build_id_index<V, const N: usize>(
        entries: &[(ChangeId, V)],
    ) -> IdIndex<ChangeId, Position, N> {
        let mut builder = IdIndex::with_capacity(entries.len());
        for (i, (k, _)) in entries.iter().enumerate() {
            builder.insert(k, Position(i));
        }
        builder.build()
    }

    #[test]
    fn test_id_index_resolve_prefix() {
        let source = vec![
            (ChangeId::from_hex("0000"), 0),
            (ChangeId::from_hex("0099"), 1),
            (ChangeId::from_hex("0099"), 2),
            (ChangeId::from_hex("0aaa"), 3),
            (ChangeId::from_hex("0aab"), 4),
        ];

        // short_key.len() == full_key.len()
        let id_index = build_id_index::<_, 2>(&source);
        let resolve_prefix = |prefix: &HexPrefix| {
            let resolution: PrefixResolution<(_, Vec<_>)> =
                id_index.resolve_prefix_with(&*source, prefix, |(_, v)| *v);
            resolution.map(|(key, mut values)| {
                values.sort(); // order of values might not be preserved by IdIndex
                (key, values)
            })
        };
        assert_eq!(
            resolve_prefix(&HexPrefix::new("0").unwrap()),
            PrefixResolution::AmbiguousMatch,
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("00").unwrap()),
            PrefixResolution::AmbiguousMatch,
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("000").unwrap()),
            PrefixResolution::SingleMatch((ChangeId::from_hex("0000"), vec![0])),
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("0001").unwrap()),
            PrefixResolution::NoMatch,
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("009").unwrap()),
            PrefixResolution::SingleMatch((ChangeId::from_hex("0099"), vec![1, 2])),
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("0aa").unwrap()),
            PrefixResolution::AmbiguousMatch,
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("0aab").unwrap()),
            PrefixResolution::SingleMatch((ChangeId::from_hex("0aab"), vec![4])),
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("f").unwrap()),
            PrefixResolution::NoMatch,
        );

        // short_key.len() < full_key.len()
        let id_index = build_id_index::<_, 1>(&source);
        let resolve_prefix = |prefix: &HexPrefix| {
            let resolution: PrefixResolution<(_, Vec<_>)> =
                id_index.resolve_prefix_with(&*source, prefix, |(_, v)| *v);
            resolution.map(|(key, mut values)| {
                values.sort(); // order of values might not be preserved by IdIndex
                (key, values)
            })
        };
        assert_eq!(
            resolve_prefix(&HexPrefix::new("00").unwrap()),
            PrefixResolution::AmbiguousMatch,
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("000").unwrap()),
            PrefixResolution::SingleMatch((ChangeId::from_hex("0000"), vec![0])),
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("0001").unwrap()),
            PrefixResolution::NoMatch,
        );
        // For short key "00", ["0000", "0099", "0099"] would match. We shouldn't
        // break at "009".matches("0000").
        assert_eq!(
            resolve_prefix(&HexPrefix::new("009").unwrap()),
            PrefixResolution::SingleMatch((ChangeId::from_hex("0099"), vec![1, 2])),
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("0a").unwrap()),
            PrefixResolution::AmbiguousMatch,
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("0aa").unwrap()),
            PrefixResolution::AmbiguousMatch,
        );
        assert_eq!(
            resolve_prefix(&HexPrefix::new("0aab").unwrap()),
            PrefixResolution::SingleMatch((ChangeId::from_hex("0aab"), vec![4])),
        );
    }

    #[test]
    fn test_lookup_exact() {
        // No crash if empty
        let source: Vec<(ChangeId, ())> = vec![];
        let id_index = build_id_index::<_, 1>(&source);
        assert!(id_index
            .lookup_exact(&*source, &ChangeId::from_hex("00"))
            .is_none());

        let source = vec![
            (ChangeId::from_hex("ab00"), ()),
            (ChangeId::from_hex("ab01"), ()),
        ];
        let id_index = build_id_index::<_, 1>(&source);
        assert!(id_index
            .lookup_exact(&*source, &ChangeId::from_hex("aa00"))
            .is_none());
        assert!(id_index
            .lookup_exact(&*source, &ChangeId::from_hex("ab00"))
            .is_some());
        assert!(id_index
            .lookup_exact(&*source, &ChangeId::from_hex("ab01"))
            .is_some());
        assert!(id_index
            .lookup_exact(&*source, &ChangeId::from_hex("ab02"))
            .is_none());
        assert!(id_index
            .lookup_exact(&*source, &ChangeId::from_hex("ac00"))
            .is_none());
    }

    #[test]
    fn test_id_index_shortest_unique_prefix_len() {
        // No crash if empty
        let source: Vec<(ChangeId, ())> = vec![];
        let id_index = build_id_index::<_, 1>(&source);
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("00")),
            1
        );

        let source = vec![
            (ChangeId::from_hex("ab"), ()),
            (ChangeId::from_hex("acd0"), ()),
            (ChangeId::from_hex("acd0"), ()), // duplicated key is allowed
        ];
        let id_index = build_id_index::<_, 1>(&source);
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("acd0")),
            2
        );
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("ac")),
            3
        );

        let source = vec![
            (ChangeId::from_hex("ab"), ()),
            (ChangeId::from_hex("acd0"), ()),
            (ChangeId::from_hex("acf0"), ()),
            (ChangeId::from_hex("a0"), ()),
            (ChangeId::from_hex("ba"), ()),
        ];
        let id_index = build_id_index::<_, 1>(&source);

        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("a0")),
            2
        );
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("ba")),
            1
        );
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("ab")),
            2
        );
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("acd0")),
            3
        );
        // If it were there, the length would be 1.
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("c0")),
            1
        );

        let source = vec![
            (ChangeId::from_hex("000000"), ()),
            (ChangeId::from_hex("01ffff"), ()),
            (ChangeId::from_hex("010000"), ()),
            (ChangeId::from_hex("01fffe"), ()),
            (ChangeId::from_hex("ffffff"), ()),
        ];
        let id_index = build_id_index::<_, 1>(&source);
        // Multiple candidates in the current chunk "01"
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("01ffff")),
            6
        );
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("010000")),
            3
        );
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("01fffe")),
            6
        );
        // Only right neighbor
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("000000")),
            2
        );
        // Only left neighbor
        assert_eq!(
            id_index.shortest_unique_prefix_len(&*source, &ChangeId::from_hex("ffffff")),
            1
        );
    }
}
