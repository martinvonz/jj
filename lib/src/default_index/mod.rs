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

mod composite;
mod entry;
mod mutable;
mod readonly;
mod rev_walk;
mod store;

pub use self::composite::{AsCompositeIndex, CompositeIndex, IndexLevelStats, IndexStats};
pub use self::entry::{IndexEntry, IndexPosition};
pub use self::mutable::DefaultMutableIndex;
pub use self::readonly::{DefaultReadonlyIndex, ReadonlyIndexLoadError};
pub use self::rev_walk::{
    RevWalk, RevWalkDescendants, RevWalkDescendantsGenerationRange, RevWalkGenerationRange,
};
pub use self::store::{DefaultIndexStore, DefaultIndexStoreError, DefaultIndexStoreInitError};

#[cfg(test)]
mod tests {
    use std::ops::Range;
    use std::sync::Arc;

    use itertools::Itertools;
    use smallvec::smallvec_inline;
    use test_case::test_case;

    use super::composite::IndexSegment;
    use super::entry::SmallIndexPositionsVec;
    use super::mutable::MutableIndexSegment;
    use super::*;
    use crate::backend::{ChangeId, CommitId, ObjectId};
    use crate::index::{HexPrefix, Index, PrefixResolution};

    /// Generator of unique 16-byte ChangeId excluding root id
    fn change_id_generator() -> impl FnMut() -> ChangeId {
        let mut iter = (1_u128..).map(|n| ChangeId::new(n.to_le_bytes().into()));
        move || iter.next().unwrap()
    }

    fn to_positions_vec(index: CompositeIndex<'_>, commit_ids: &[CommitId]) -> Vec<IndexPosition> {
        commit_ids
            .iter()
            .map(|id| index.commit_id_to_pos(id).unwrap())
            .collect()
    }

    #[test_case(false; "memory")]
    #[test_case(true; "file")]
    fn index_empty(on_disk: bool) {
        let temp_dir = testutils::new_temp_dir();
        let mutable_segment = MutableIndexSegment::full(3, 16);
        let index_segment: Box<dyn IndexSegment> = if on_disk {
            let saved_index = mutable_segment.save_in(temp_dir.path()).unwrap();
            Box::new(Arc::try_unwrap(saved_index).unwrap())
        } else {
            Box::new(mutable_segment)
        };
        let index = CompositeIndex::new(index_segment.as_ref());

        // Stats are as expected
        let stats = index.stats();
        assert_eq!(stats.num_commits, 0);
        assert_eq!(stats.num_heads, 0);
        assert_eq!(stats.max_generation_number, 0);
        assert_eq!(stats.num_merges, 0);
        assert_eq!(stats.num_changes, 0);
        assert_eq!(index.num_commits(), 0);
        // Cannot find any commits
        assert!(index.entry_by_id(&CommitId::from_hex("000000")).is_none());
        assert!(index.entry_by_id(&CommitId::from_hex("aaa111")).is_none());
        assert!(index.entry_by_id(&CommitId::from_hex("ffffff")).is_none());
    }

    #[test_case(false; "memory")]
    #[test_case(true; "file")]
    fn index_root_commit(on_disk: bool) {
        let temp_dir = testutils::new_temp_dir();
        let mut new_change_id = change_id_generator();
        let mut mutable_segment = MutableIndexSegment::full(3, 16);
        let id_0 = CommitId::from_hex("000000");
        let change_id0 = new_change_id();
        mutable_segment.add_commit_data(id_0.clone(), change_id0.clone(), &[]);
        let index_segment: Box<dyn IndexSegment> = if on_disk {
            let saved_index = mutable_segment.save_in(temp_dir.path()).unwrap();
            Box::new(Arc::try_unwrap(saved_index).unwrap())
        } else {
            Box::new(mutable_segment)
        };
        let index = CompositeIndex::new(index_segment.as_ref());

        // Stats are as expected
        let stats = index.stats();
        assert_eq!(stats.num_commits, 1);
        assert_eq!(stats.num_heads, 1);
        assert_eq!(stats.max_generation_number, 0);
        assert_eq!(stats.num_merges, 0);
        assert_eq!(stats.num_changes, 1);
        assert_eq!(index.num_commits(), 1);
        // Can find only the root commit
        assert_eq!(index.commit_id_to_pos(&id_0), Some(IndexPosition(0)));
        assert_eq!(index.commit_id_to_pos(&CommitId::from_hex("aaaaaa")), None);
        assert_eq!(index.commit_id_to_pos(&CommitId::from_hex("ffffff")), None);
        // Check properties of root entry
        let entry = index.entry_by_id(&id_0).unwrap();
        assert_eq!(entry.position(), IndexPosition(0));
        assert_eq!(entry.commit_id(), id_0);
        assert_eq!(entry.change_id(), change_id0);
        assert_eq!(entry.generation_number(), 0);
        assert_eq!(entry.num_parents(), 0);
        assert_eq!(entry.parent_positions(), SmallIndexPositionsVec::new());
        assert_eq!(entry.parents().len(), 0);
    }

    #[test]
    #[should_panic(expected = "parent commit is not indexed")]
    fn index_missing_parent_commit() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        index.add_commit_data(id_1, new_change_id(), &[id_0]);
    }

    #[test_case(false, false; "full in memory")]
    #[test_case(false, true; "full on disk")]
    #[test_case(true, false; "incremental in memory")]
    #[test_case(true, true; "incremental on disk")]
    fn index_multiple_commits(incremental: bool, on_disk: bool) {
        let temp_dir = testutils::new_temp_dir();
        let mut new_change_id = change_id_generator();
        let mut mutable_segment = MutableIndexSegment::full(3, 16);
        // 5
        // |\
        // 4 | 3
        // | |/
        // 1 2
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let change_id0 = new_change_id();
        let id_1 = CommitId::from_hex("111111");
        let change_id1 = new_change_id();
        let id_2 = CommitId::from_hex("222222");
        #[allow(clippy::redundant_clone)] // Work around nightly clippy false positive
        // TODO: Remove the exception after https://github.com/rust-lang/rust-clippy/issues/10577
        // is fixed or file a new bug.
        let change_id2 = change_id1.clone();
        mutable_segment.add_commit_data(id_0.clone(), change_id0, &[]);
        mutable_segment.add_commit_data(id_1.clone(), change_id1.clone(), &[id_0.clone()]);
        mutable_segment.add_commit_data(id_2.clone(), change_id2.clone(), &[id_0.clone()]);

        // If testing incremental indexing, write the first three commits to one file
        // now and build the remainder as another segment on top.
        if incremental {
            let initial_file = mutable_segment.save_in(temp_dir.path()).unwrap();
            mutable_segment = MutableIndexSegment::incremental(initial_file);
        }

        let id_3 = CommitId::from_hex("333333");
        let change_id3 = new_change_id();
        let id_4 = CommitId::from_hex("444444");
        let change_id4 = new_change_id();
        let id_5 = CommitId::from_hex("555555");
        let change_id5 = change_id3.clone();
        mutable_segment.add_commit_data(id_3.clone(), change_id3.clone(), &[id_2.clone()]);
        mutable_segment.add_commit_data(id_4.clone(), change_id4, &[id_1.clone()]);
        mutable_segment.add_commit_data(id_5.clone(), change_id5, &[id_4.clone(), id_2.clone()]);
        let index_segment: Box<dyn IndexSegment> = if on_disk {
            let saved_index = mutable_segment.save_in(temp_dir.path()).unwrap();
            Box::new(Arc::try_unwrap(saved_index).unwrap())
        } else {
            Box::new(mutable_segment)
        };
        let index = CompositeIndex::new(index_segment.as_ref());

        // Stats are as expected
        let stats = index.stats();
        assert_eq!(stats.num_commits, 6);
        assert_eq!(stats.num_heads, 2);
        assert_eq!(stats.max_generation_number, 3);
        assert_eq!(stats.num_merges, 1);
        assert_eq!(stats.num_changes, 4);
        assert_eq!(index.num_commits(), 6);
        // Can find all the commits
        let entry_0 = index.entry_by_id(&id_0).unwrap();
        let entry_1 = index.entry_by_id(&id_1).unwrap();
        let entry_2 = index.entry_by_id(&id_2).unwrap();
        let entry_3 = index.entry_by_id(&id_3).unwrap();
        let entry_4 = index.entry_by_id(&id_4).unwrap();
        let entry_5 = index.entry_by_id(&id_5).unwrap();
        // Check properties of some entries
        assert_eq!(entry_0.position(), IndexPosition(0));
        assert_eq!(entry_0.commit_id(), id_0);
        assert_eq!(entry_1.position(), IndexPosition(1));
        assert_eq!(entry_1.commit_id(), id_1);
        assert_eq!(entry_1.change_id(), change_id1);
        assert_eq!(entry_1.generation_number(), 1);
        assert_eq!(entry_1.num_parents(), 1);
        assert_eq!(
            entry_1.parent_positions(),
            smallvec_inline![IndexPosition(0)]
        );
        assert_eq!(entry_1.parents().len(), 1);
        assert_eq!(
            entry_1.parents().next().unwrap().position(),
            IndexPosition(0)
        );
        assert_eq!(entry_2.position(), IndexPosition(2));
        assert_eq!(entry_2.commit_id(), id_2);
        assert_eq!(entry_2.change_id(), change_id2);
        assert_eq!(entry_2.generation_number(), 1);
        assert_eq!(entry_2.num_parents(), 1);
        assert_eq!(
            entry_2.parent_positions(),
            smallvec_inline![IndexPosition(0)]
        );
        assert_eq!(entry_3.change_id(), change_id3);
        assert_eq!(entry_3.generation_number(), 2);
        assert_eq!(
            entry_3.parent_positions(),
            smallvec_inline![IndexPosition(2)]
        );
        assert_eq!(entry_4.position(), IndexPosition(4));
        assert_eq!(entry_4.generation_number(), 2);
        assert_eq!(entry_4.num_parents(), 1);
        assert_eq!(
            entry_4.parent_positions(),
            smallvec_inline![IndexPosition(1)]
        );
        assert_eq!(entry_5.generation_number(), 3);
        assert_eq!(entry_5.num_parents(), 2);
        assert_eq!(
            entry_5.parent_positions(),
            smallvec_inline![IndexPosition(4), IndexPosition(2)]
        );
        assert_eq!(entry_5.parents().len(), 2);
        assert_eq!(
            entry_5.parents().next().unwrap().position(),
            IndexPosition(4)
        );
        assert_eq!(
            entry_5.parents().nth(1).unwrap().position(),
            IndexPosition(2)
        );
    }

    #[test_case(false; "in memory")]
    #[test_case(true; "on disk")]
    fn index_many_parents(on_disk: bool) {
        let temp_dir = testutils::new_temp_dir();
        let mut new_change_id = change_id_generator();
        let mut mutable_segment = MutableIndexSegment::full(3, 16);
        //     6
        //    /|\
        //   / | \
        //  / /|\ \
        // 1 2 3 4 5
        //  \ \|/ /
        //   \ | /
        //    \|/
        //     0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        let id_6 = CommitId::from_hex("666666");
        mutable_segment.add_commit_data(id_0.clone(), new_change_id(), &[]);
        mutable_segment.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        mutable_segment.add_commit_data(id_2.clone(), new_change_id(), &[id_0.clone()]);
        mutable_segment.add_commit_data(id_3.clone(), new_change_id(), &[id_0.clone()]);
        mutable_segment.add_commit_data(id_4.clone(), new_change_id(), &[id_0.clone()]);
        mutable_segment.add_commit_data(id_5.clone(), new_change_id(), &[id_0]);
        mutable_segment.add_commit_data(
            id_6.clone(),
            new_change_id(),
            &[id_1, id_2, id_3, id_4, id_5],
        );
        let index_segment: Box<dyn IndexSegment> = if on_disk {
            let saved_index = mutable_segment.save_in(temp_dir.path()).unwrap();
            Box::new(Arc::try_unwrap(saved_index).unwrap())
        } else {
            Box::new(mutable_segment)
        };
        let index = CompositeIndex::new(index_segment.as_ref());

        // Stats are as expected
        let stats = index.stats();
        assert_eq!(stats.num_commits, 7);
        assert_eq!(stats.num_heads, 1);
        assert_eq!(stats.max_generation_number, 2);
        assert_eq!(stats.num_merges, 1);

        // The octopus merge has the right parents
        let entry_6 = index.entry_by_id(&id_6).unwrap();
        assert_eq!(entry_6.commit_id(), id_6.clone());
        assert_eq!(entry_6.num_parents(), 5);
        assert_eq!(
            entry_6.parent_positions(),
            smallvec_inline![
                IndexPosition(1),
                IndexPosition(2),
                IndexPosition(3),
                IndexPosition(4),
                IndexPosition(5),
            ]
        );
        assert_eq!(entry_6.generation_number(), 2);
    }

    #[test]
    fn resolve_prefix() {
        let temp_dir = testutils::new_temp_dir();
        let mut new_change_id = change_id_generator();
        let mut mutable_segment = MutableIndexSegment::full(3, 16);

        // Create some commits with different various common prefixes.
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("009999");
        let id_2 = CommitId::from_hex("055488");
        mutable_segment.add_commit_data(id_0.clone(), new_change_id(), &[]);
        mutable_segment.add_commit_data(id_1.clone(), new_change_id(), &[]);
        mutable_segment.add_commit_data(id_2.clone(), new_change_id(), &[]);

        // Write the first three commits to one file and build the remainder on top.
        let initial_file = mutable_segment.save_in(temp_dir.path()).unwrap();
        mutable_segment = MutableIndexSegment::incremental(initial_file);

        let id_3 = CommitId::from_hex("055444");
        let id_4 = CommitId::from_hex("055555");
        let id_5 = CommitId::from_hex("033333");
        mutable_segment.add_commit_data(id_3, new_change_id(), &[]);
        mutable_segment.add_commit_data(id_4, new_change_id(), &[]);
        mutable_segment.add_commit_data(id_5, new_change_id(), &[]);

        let index = mutable_segment.as_composite();

        // Can find commits given the full hex number
        assert_eq!(
            index.resolve_commit_id_prefix(&HexPrefix::new(&id_0.hex()).unwrap()),
            PrefixResolution::SingleMatch(id_0)
        );
        assert_eq!(
            index.resolve_commit_id_prefix(&HexPrefix::new(&id_1.hex()).unwrap()),
            PrefixResolution::SingleMatch(id_1)
        );
        assert_eq!(
            index.resolve_commit_id_prefix(&HexPrefix::new(&id_2.hex()).unwrap()),
            PrefixResolution::SingleMatch(id_2)
        );
        // Test nonexistent commits
        assert_eq!(
            index.resolve_commit_id_prefix(&HexPrefix::new("ffffff").unwrap()),
            PrefixResolution::NoMatch
        );
        assert_eq!(
            index.resolve_commit_id_prefix(&HexPrefix::new("000001").unwrap()),
            PrefixResolution::NoMatch
        );
        // Test ambiguous prefix
        assert_eq!(
            index.resolve_commit_id_prefix(&HexPrefix::new("0").unwrap()),
            PrefixResolution::AmbiguousMatch
        );
        // Test a globally unique prefix in initial part
        assert_eq!(
            index.resolve_commit_id_prefix(&HexPrefix::new("009").unwrap()),
            PrefixResolution::SingleMatch(CommitId::from_hex("009999"))
        );
        // Test a globally unique prefix in incremental part
        assert_eq!(
            index.resolve_commit_id_prefix(&HexPrefix::new("03").unwrap()),
            PrefixResolution::SingleMatch(CommitId::from_hex("033333"))
        );
        // Test a locally unique but globally ambiguous prefix
        assert_eq!(
            index.resolve_commit_id_prefix(&HexPrefix::new("0554").unwrap()),
            PrefixResolution::AmbiguousMatch
        );
    }

    #[test]
    #[allow(clippy::redundant_clone)] // allow id_n.clone()
    fn neighbor_commit_ids() {
        let temp_dir = testutils::new_temp_dir();
        let mut new_change_id = change_id_generator();
        let mut mutable_segment = MutableIndexSegment::full(3, 16);

        // Create some commits with different various common prefixes.
        let id_0 = CommitId::from_hex("000001");
        let id_1 = CommitId::from_hex("009999");
        let id_2 = CommitId::from_hex("055488");
        mutable_segment.add_commit_data(id_0.clone(), new_change_id(), &[]);
        mutable_segment.add_commit_data(id_1.clone(), new_change_id(), &[]);
        mutable_segment.add_commit_data(id_2.clone(), new_change_id(), &[]);

        // Write the first three commits to one file and build the remainder on top.
        let initial_file = mutable_segment.save_in(temp_dir.path()).unwrap();
        mutable_segment = MutableIndexSegment::incremental(initial_file.clone());

        let id_3 = CommitId::from_hex("055444");
        let id_4 = CommitId::from_hex("055555");
        let id_5 = CommitId::from_hex("033333");
        mutable_segment.add_commit_data(id_3.clone(), new_change_id(), &[]);
        mutable_segment.add_commit_data(id_4.clone(), new_change_id(), &[]);
        mutable_segment.add_commit_data(id_5.clone(), new_change_id(), &[]);

        // Local lookup in readonly index, commit_id exists.
        assert_eq!(
            initial_file.resolve_neighbor_commit_ids(&id_0),
            (None, Some(id_1.clone())),
        );
        assert_eq!(
            initial_file.resolve_neighbor_commit_ids(&id_1),
            (Some(id_0.clone()), Some(id_2.clone())),
        );
        assert_eq!(
            initial_file.resolve_neighbor_commit_ids(&id_2),
            (Some(id_1.clone()), None),
        );

        // Local lookup in readonly index, commit_id does not exist.
        assert_eq!(
            initial_file.resolve_neighbor_commit_ids(&CommitId::from_hex("000000")),
            (None, Some(id_0.clone())),
        );
        assert_eq!(
            initial_file.resolve_neighbor_commit_ids(&CommitId::from_hex("000002")),
            (Some(id_0.clone()), Some(id_1.clone())),
        );
        assert_eq!(
            initial_file.resolve_neighbor_commit_ids(&CommitId::from_hex("ffffff")),
            (Some(id_2.clone()), None),
        );

        // Local lookup in mutable index, commit_id exists. id_5 < id_3 < id_4
        assert_eq!(
            mutable_segment.resolve_neighbor_commit_ids(&id_5),
            (None, Some(id_3.clone())),
        );
        assert_eq!(
            mutable_segment.resolve_neighbor_commit_ids(&id_3),
            (Some(id_5.clone()), Some(id_4.clone())),
        );
        assert_eq!(
            mutable_segment.resolve_neighbor_commit_ids(&id_4),
            (Some(id_3.clone()), None),
        );

        // Local lookup in mutable index, commit_id does not exist. id_5 < id_3 < id_4
        assert_eq!(
            mutable_segment.resolve_neighbor_commit_ids(&CommitId::from_hex("033332")),
            (None, Some(id_5.clone())),
        );
        assert_eq!(
            mutable_segment.resolve_neighbor_commit_ids(&CommitId::from_hex("033334")),
            (Some(id_5.clone()), Some(id_3.clone())),
        );
        assert_eq!(
            mutable_segment.resolve_neighbor_commit_ids(&CommitId::from_hex("ffffff")),
            (Some(id_4.clone()), None),
        );

        // Global lookup, commit_id exists. id_0 < id_1 < id_5 < id_3 < id_2 < id_4
        let composite_index = CompositeIndex::new(&mutable_segment);
        assert_eq!(
            composite_index.resolve_neighbor_commit_ids(&id_0),
            (None, Some(id_1.clone())),
        );
        assert_eq!(
            composite_index.resolve_neighbor_commit_ids(&id_1),
            (Some(id_0.clone()), Some(id_5.clone())),
        );
        assert_eq!(
            composite_index.resolve_neighbor_commit_ids(&id_5),
            (Some(id_1.clone()), Some(id_3.clone())),
        );
        assert_eq!(
            composite_index.resolve_neighbor_commit_ids(&id_3),
            (Some(id_5.clone()), Some(id_2.clone())),
        );
        assert_eq!(
            composite_index.resolve_neighbor_commit_ids(&id_2),
            (Some(id_3.clone()), Some(id_4.clone())),
        );
        assert_eq!(
            composite_index.resolve_neighbor_commit_ids(&id_4),
            (Some(id_2.clone()), None),
        );

        // Global lookup, commit_id doesn't exist. id_0 < id_1 < id_5 < id_3 < id_2 <
        // id_4
        assert_eq!(
            composite_index.resolve_neighbor_commit_ids(&CommitId::from_hex("000000")),
            (None, Some(id_0.clone())),
        );
        assert_eq!(
            composite_index.resolve_neighbor_commit_ids(&CommitId::from_hex("010000")),
            (Some(id_1.clone()), Some(id_5.clone())),
        );
        assert_eq!(
            composite_index.resolve_neighbor_commit_ids(&CommitId::from_hex("033334")),
            (Some(id_5.clone()), Some(id_3.clone())),
        );
        assert_eq!(
            composite_index.resolve_neighbor_commit_ids(&CommitId::from_hex("ffffff")),
            (Some(id_4.clone()), None),
        );
    }

    #[test]
    fn shortest_unique_commit_id_prefix() {
        let temp_dir = testutils::new_temp_dir();
        let mut new_change_id = change_id_generator();
        let mut mutable_segment = MutableIndexSegment::full(3, 16);

        // Create some commits with different various common prefixes.
        let id_0 = CommitId::from_hex("000001");
        let id_1 = CommitId::from_hex("009999");
        let id_2 = CommitId::from_hex("055488");
        mutable_segment.add_commit_data(id_0.clone(), new_change_id(), &[]);
        mutable_segment.add_commit_data(id_1.clone(), new_change_id(), &[]);
        mutable_segment.add_commit_data(id_2.clone(), new_change_id(), &[]);

        // Write the first three commits to one file and build the remainder on top.
        let initial_file = mutable_segment.save_in(temp_dir.path()).unwrap();
        mutable_segment = MutableIndexSegment::incremental(initial_file);

        let id_3 = CommitId::from_hex("055444");
        let id_4 = CommitId::from_hex("055555");
        let id_5 = CommitId::from_hex("033333");
        mutable_segment.add_commit_data(id_3.clone(), new_change_id(), &[]);
        mutable_segment.add_commit_data(id_4.clone(), new_change_id(), &[]);
        mutable_segment.add_commit_data(id_5.clone(), new_change_id(), &[]);

        let index = mutable_segment.as_composite();

        // Public API: calculate shortest unique prefix len with known commit_id
        assert_eq!(index.shortest_unique_commit_id_prefix_len(&id_0), 3);
        assert_eq!(index.shortest_unique_commit_id_prefix_len(&id_1), 3);
        assert_eq!(index.shortest_unique_commit_id_prefix_len(&id_2), 5);
        assert_eq!(index.shortest_unique_commit_id_prefix_len(&id_3), 5);
        assert_eq!(index.shortest_unique_commit_id_prefix_len(&id_4), 4);
        assert_eq!(index.shortest_unique_commit_id_prefix_len(&id_5), 2);

        // Public API: calculate shortest unique prefix len with unknown commit_id
        assert_eq!(
            index.shortest_unique_commit_id_prefix_len(&CommitId::from_hex("000002")),
            6
        );
        assert_eq!(
            index.shortest_unique_commit_id_prefix_len(&CommitId::from_hex("010000")),
            2
        );
        assert_eq!(
            index.shortest_unique_commit_id_prefix_len(&CommitId::from_hex("033334")),
            6
        );
        assert_eq!(
            index.shortest_unique_commit_id_prefix_len(&CommitId::from_hex("ffffff")),
            1
        );
    }

    #[test]
    fn test_is_ancestor() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // 5
        // |\
        // 4 | 3
        // | |/
        // 1 2
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_2.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_5.clone(), new_change_id(), &[id_4.clone(), id_2.clone()]);

        assert!(index.is_ancestor(&id_0, &id_0));
        assert!(index.is_ancestor(&id_0, &id_1));
        assert!(index.is_ancestor(&id_2, &id_3));
        assert!(index.is_ancestor(&id_2, &id_5));
        assert!(index.is_ancestor(&id_1, &id_5));
        assert!(index.is_ancestor(&id_0, &id_5));
        assert!(!index.is_ancestor(&id_1, &id_0));
        assert!(!index.is_ancestor(&id_5, &id_3));
        assert!(!index.is_ancestor(&id_3, &id_5));
        assert!(!index.is_ancestor(&id_2, &id_4));
        assert!(!index.is_ancestor(&id_4, &id_2));
    }

    #[test]
    fn test_common_ancestors() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // 5
        // |\
        // 4 |
        // | |
        // 1 2 3
        // | |/
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_5.clone(), new_change_id(), &[id_4.clone(), id_2.clone()]);

        assert_eq!(
            index.common_ancestors(&[id_0.clone()], &[id_0.clone()]),
            vec![id_0.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_5.clone()], &[id_5.clone()]),
            vec![id_5.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_1.clone()], &[id_2.clone()]),
            vec![id_0.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_2.clone()], &[id_1.clone()]),
            vec![id_0.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_1.clone()], &[id_4.clone()]),
            vec![id_1.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_4.clone()], &[id_1.clone()]),
            vec![id_1.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_3.clone()], &[id_5.clone()]),
            vec![id_0.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_5.clone()], &[id_3.clone()]),
            vec![id_0.clone()]
        );

        // With multiple commits in an input set
        assert_eq!(
            index.common_ancestors(&[id_0.clone(), id_1.clone()], &[id_0.clone()]),
            vec![id_0.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_0.clone(), id_1.clone()], &[id_1.clone()]),
            vec![id_1.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_1.clone(), id_2.clone()], &[id_1.clone()]),
            vec![id_1.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_1.clone(), id_2.clone()], &[id_4]),
            vec![id_1.clone()]
        );
        assert_eq!(
            index.common_ancestors(&[id_1.clone(), id_2.clone()], &[id_5]),
            vec![id_1.clone(), id_2.clone()]
        );
        assert_eq!(index.common_ancestors(&[id_1, id_2], &[id_3]), vec![id_0]);
    }

    #[test]
    fn test_common_ancestors_criss_cross() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // 3 4
        // |X|
        // 1 2
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_0]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_1.clone(), id_2.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_1.clone(), id_2.clone()]);

        let mut common_ancestors = index.common_ancestors(&[id_3], &[id_4]);
        common_ancestors.sort();
        assert_eq!(common_ancestors, vec![id_1, id_2]);
    }

    #[test]
    fn test_common_ancestors_merge_with_ancestor() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // 4   5
        // |\ /|
        // 1 2 3
        //  \|/
        //   0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1, new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_3, new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_0.clone(), id_2.clone()]);
        index.add_commit_data(id_5.clone(), new_change_id(), &[id_0, id_2.clone()]);

        let mut common_ancestors = index.common_ancestors(&[id_4], &[id_5]);
        common_ancestors.sort();
        assert_eq!(common_ancestors, vec![id_2]);
    }

    #[test]
    fn test_walk_revs() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // 5
        // |\
        // 4 | 3
        // | |/
        // 1 2
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_2.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_5.clone(), new_change_id(), &[id_4.clone(), id_2.clone()]);

        let walk_commit_ids = |wanted: &[CommitId], unwanted: &[CommitId]| {
            let index = index.as_composite();
            let wanted_positions = to_positions_vec(index, wanted);
            let unwanted_positions = to_positions_vec(index, unwanted);
            index
                .walk_revs(&wanted_positions, &unwanted_positions)
                .map(|entry| entry.commit_id())
                .collect_vec()
        };

        // No wanted commits
        assert!(walk_commit_ids(&[], &[]).is_empty());
        // Simple linear walk to roo
        assert_eq!(
            walk_commit_ids(&[id_4.clone()], &[]),
            vec![id_4.clone(), id_1.clone(), id_0.clone()]
        );
        // Commits that are both wanted and unwanted are not walked
        assert_eq!(walk_commit_ids(&[id_0.clone()], &[id_0.clone()]), vec![]);
        // Commits that are listed twice are only walked once
        assert_eq!(
            walk_commit_ids(&[id_0.clone(), id_0.clone()], &[]),
            vec![id_0.clone()]
        );
        // If a commit and its ancestor are both wanted, the ancestor still gets walked
        // only once
        assert_eq!(
            walk_commit_ids(&[id_0.clone(), id_1.clone()], &[]),
            vec![id_1.clone(), id_0.clone()]
        );
        // Ancestors of both wanted and unwanted commits are not walked
        assert_eq!(
            walk_commit_ids(&[id_2.clone()], &[id_1.clone()]),
            vec![id_2.clone()]
        );
        // Same as above, but the opposite order, to make sure that order in index
        // doesn't matter
        assert_eq!(
            walk_commit_ids(&[id_1.clone()], &[id_2.clone()]),
            vec![id_1.clone()]
        );
        // Two wanted nodes
        assert_eq!(
            walk_commit_ids(&[id_1.clone(), id_2.clone()], &[]),
            vec![id_2.clone(), id_1.clone(), id_0.clone()]
        );
        // Order of output doesn't depend on order of input
        assert_eq!(
            walk_commit_ids(&[id_2.clone(), id_1.clone()], &[]),
            vec![id_2.clone(), id_1.clone(), id_0]
        );
        // Two wanted nodes that share an unwanted ancestor
        assert_eq!(
            walk_commit_ids(&[id_5.clone(), id_3.clone()], &[id_2]),
            vec![id_5, id_4, id_3, id_1]
        );
    }

    #[test]
    fn test_walk_revs_filter_by_generation() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // 8 6
        // | |
        // 7 5
        // |/|
        // 4 |
        // | 3
        // 2 |
        // |/
        // 1
        // |
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        let id_6 = CommitId::from_hex("666666");
        let id_7 = CommitId::from_hex("777777");
        let id_8 = CommitId::from_hex("888888");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_2.clone()]);
        index.add_commit_data(id_5.clone(), new_change_id(), &[id_4.clone(), id_3.clone()]);
        index.add_commit_data(id_6.clone(), new_change_id(), &[id_5.clone()]);
        index.add_commit_data(id_7.clone(), new_change_id(), &[id_4.clone()]);
        index.add_commit_data(id_8.clone(), new_change_id(), &[id_7.clone()]);

        let walk_commit_ids = |wanted: &[CommitId], unwanted: &[CommitId], range: Range<u32>| {
            let index = index.as_composite();
            let wanted_positions = to_positions_vec(index, wanted);
            let unwanted_positions = to_positions_vec(index, unwanted);
            index
                .walk_revs(&wanted_positions, &unwanted_positions)
                .filter_by_generation(range)
                .map(|entry| entry.commit_id())
                .collect_vec()
        };

        // Empty generation bounds
        assert_eq!(walk_commit_ids(&[&id_8].map(Clone::clone), &[], 0..0), []);
        assert_eq!(
            walk_commit_ids(&[&id_8].map(Clone::clone), &[], Range { start: 2, end: 1 }),
            []
        );

        // Simple generation bounds
        assert_eq!(
            walk_commit_ids(&[&id_2].map(Clone::clone), &[], 0..3),
            [&id_2, &id_1, &id_0].map(Clone::clone)
        );

        // Ancestors may be walked with different generations
        assert_eq!(
            walk_commit_ids(&[&id_6].map(Clone::clone), &[], 2..4),
            [&id_4, &id_3, &id_2, &id_1].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_5].map(Clone::clone), &[], 2..3),
            [&id_2, &id_1].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_5, &id_7].map(Clone::clone), &[], 2..3),
            [&id_2, &id_1].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_7, &id_8].map(Clone::clone), &[], 0..2),
            [&id_8, &id_7, &id_4].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_6, &id_7].map(Clone::clone), &[], 0..3),
            [&id_7, &id_6, &id_5, &id_4, &id_3, &id_2].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_6, &id_7].map(Clone::clone), &[], 2..3),
            [&id_4, &id_3, &id_2].map(Clone::clone)
        );

        // Ancestors of both wanted and unwanted commits are not walked
        assert_eq!(
            walk_commit_ids(&[&id_5].map(Clone::clone), &[&id_2].map(Clone::clone), 1..5),
            [&id_4, &id_3].map(Clone::clone)
        );
    }

    #[test]
    #[allow(clippy::redundant_clone)] // allow id_n.clone()
    fn test_walk_revs_filter_by_generation_range_merging() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // Long linear history with some short branches
        let ids = (0..11)
            .map(|n| CommitId::from_hex(&format!("{n:06x}")))
            .collect_vec();
        index.add_commit_data(ids[0].clone(), new_change_id(), &[]);
        for i in 1..ids.len() {
            index.add_commit_data(ids[i].clone(), new_change_id(), &[ids[i - 1].clone()]);
        }
        let id_branch5_0 = CommitId::from_hex("050000");
        let id_branch5_1 = CommitId::from_hex("050001");
        index.add_commit_data(id_branch5_0.clone(), new_change_id(), &[ids[5].clone()]);
        index.add_commit_data(
            id_branch5_1.clone(),
            new_change_id(),
            &[id_branch5_0.clone()],
        );

        let walk_commit_ids = |wanted: &[CommitId], range: Range<u32>| {
            let index = index.as_composite();
            let wanted_positions = to_positions_vec(index, wanted);
            index
                .walk_revs(&wanted_positions, &[])
                .filter_by_generation(range)
                .map(|entry| entry.commit_id())
                .collect_vec()
        };

        // Multiple non-overlapping generation ranges to track:
        // 9->6: 3..5, 6: 0..2
        assert_eq!(
            walk_commit_ids(&[&ids[9], &ids[6]].map(Clone::clone), 4..6),
            [&ids[5], &ids[4], &ids[2], &ids[1]].map(Clone::clone)
        );

        // Multiple non-overlapping generation ranges to track, and merged later:
        // 10->7: 3..5, 7: 0..2
        // 10->6: 4..6, 7->6, 1..3, 6: 0..2
        assert_eq!(
            walk_commit_ids(&[&ids[10], &ids[7], &ids[6]].map(Clone::clone), 5..7),
            [&ids[5], &ids[4], &ids[2], &ids[1], &ids[0]].map(Clone::clone)
        );

        // Merge range with sub-range (1..4 + 2..3 should be 1..4, not 1..3):
        // 8,7,6->5::1..4, B5_1->5::2..3
        assert_eq!(
            walk_commit_ids(
                &[&ids[8], &ids[7], &ids[6], &id_branch5_1].map(Clone::clone),
                5..6
            ),
            [&ids[3], &ids[2], &ids[1]].map(Clone::clone)
        );
    }

    #[test]
    fn test_walk_revs_descendants_filtered_by_generation() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // 8 6
        // | |
        // 7 5
        // |/|
        // 4 |
        // | 3
        // 2 |
        // |/
        // 1
        // |
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        let id_6 = CommitId::from_hex("666666");
        let id_7 = CommitId::from_hex("777777");
        let id_8 = CommitId::from_hex("888888");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_2.clone()]);
        index.add_commit_data(id_5.clone(), new_change_id(), &[id_4.clone(), id_3.clone()]);
        index.add_commit_data(id_6.clone(), new_change_id(), &[id_5.clone()]);
        index.add_commit_data(id_7.clone(), new_change_id(), &[id_4.clone()]);
        index.add_commit_data(id_8.clone(), new_change_id(), &[id_7.clone()]);

        let visible_heads = [&id_6, &id_8].map(Clone::clone);
        let walk_commit_ids = |roots: &[CommitId], heads: &[CommitId], range: Range<u32>| {
            let index = index.as_composite();
            let root_positions = to_positions_vec(index, roots);
            let head_positions = to_positions_vec(index, heads);
            index
                .walk_revs(&head_positions, &[])
                .descendants_filtered_by_generation(&root_positions, range)
                .map(|entry| entry.commit_id())
                .collect_vec()
        };

        // Empty generation bounds
        assert_eq!(
            walk_commit_ids(&[&id_0].map(Clone::clone), &visible_heads, 0..0),
            []
        );
        assert_eq!(
            walk_commit_ids(
                &[&id_8].map(Clone::clone),
                &visible_heads,
                Range { start: 2, end: 1 }
            ),
            []
        );

        // Full generation bounds
        assert_eq!(
            walk_commit_ids(&[&id_0].map(Clone::clone), &visible_heads, 0..u32::MAX),
            [&id_0, &id_1, &id_2, &id_3, &id_4, &id_5, &id_6, &id_7, &id_8].map(Clone::clone)
        );

        // Simple generation bounds
        assert_eq!(
            walk_commit_ids(&[&id_3].map(Clone::clone), &visible_heads, 0..3),
            [&id_3, &id_5, &id_6].map(Clone::clone)
        );

        // Descendants may be walked with different generations
        assert_eq!(
            walk_commit_ids(&[&id_0].map(Clone::clone), &visible_heads, 2..4),
            [&id_2, &id_3, &id_4, &id_5].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_1].map(Clone::clone), &visible_heads, 2..3),
            [&id_4, &id_5].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_2, &id_3].map(Clone::clone), &visible_heads, 2..3),
            [&id_5, &id_6, &id_7].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_2, &id_4].map(Clone::clone), &visible_heads, 0..2),
            [&id_2, &id_4, &id_5, &id_7].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_2, &id_3].map(Clone::clone), &visible_heads, 0..3),
            [&id_2, &id_3, &id_4, &id_5, &id_6, &id_7].map(Clone::clone)
        );
        assert_eq!(
            walk_commit_ids(&[&id_2, &id_3].map(Clone::clone), &visible_heads, 2..3),
            [&id_5, &id_6, &id_7].map(Clone::clone)
        );

        // Roots set contains entries unreachable from heads
        assert_eq!(
            walk_commit_ids(
                &[&id_2, &id_3].map(Clone::clone),
                &[&id_8].map(Clone::clone),
                0..3
            ),
            [&id_2, &id_4, &id_7].map(Clone::clone)
        );
    }

    #[test]
    fn test_heads() {
        let mut new_change_id = change_id_generator();
        let mut index = DefaultMutableIndex::full(3, 16);
        // 5
        // |\
        // 4 | 3
        // | |/
        // 1 2
        // |/
        // 0
        let id_0 = CommitId::from_hex("000000");
        let id_1 = CommitId::from_hex("111111");
        let id_2 = CommitId::from_hex("222222");
        let id_3 = CommitId::from_hex("333333");
        let id_4 = CommitId::from_hex("444444");
        let id_5 = CommitId::from_hex("555555");
        index.add_commit_data(id_0.clone(), new_change_id(), &[]);
        index.add_commit_data(id_1.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_2.clone(), new_change_id(), &[id_0.clone()]);
        index.add_commit_data(id_3.clone(), new_change_id(), &[id_2.clone()]);
        index.add_commit_data(id_4.clone(), new_change_id(), &[id_1.clone()]);
        index.add_commit_data(id_5.clone(), new_change_id(), &[id_4.clone(), id_2.clone()]);

        // Empty input
        assert!(index.heads(&mut [].iter()).is_empty());
        // Single head
        assert_eq!(index.heads(&mut [id_4.clone()].iter()), vec![id_4.clone()]);
        // Single head and parent
        assert_eq!(
            index.heads(&mut [id_4.clone(), id_1].iter()),
            vec![id_4.clone()]
        );
        // Single head and grand-parent
        assert_eq!(
            index.heads(&mut [id_4.clone(), id_0].iter()),
            vec![id_4.clone()]
        );
        // Multiple heads
        assert_eq!(
            index.heads(&mut [id_4.clone(), id_3.clone()].iter()),
            vec![id_3.clone(), id_4]
        );
        // Merge commit and ancestors
        assert_eq!(
            index.heads(&mut [id_5.clone(), id_2].iter()),
            vec![id_5.clone()]
        );
        // Merge commit and other commit
        assert_eq!(
            index.heads(&mut [id_5.clone(), id_3.clone()].iter()),
            vec![id_3, id_5]
        );
    }
}
