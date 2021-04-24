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

use std::collections::{BTreeMap, HashSet};

use crate::index::{IndexEntry, IndexPosition};
use crate::revset::RevsetIterator;

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct RevsetGraphEdge {
    pub target: IndexPosition,
    pub edge_type: RevsetGraphEdgeType,
}

impl RevsetGraphEdge {
    pub fn missing(target: IndexPosition) -> Self {
        Self {
            target,
            edge_type: RevsetGraphEdgeType::Missing,
        }
    }
    pub fn direct(target: IndexPosition) -> Self {
        Self {
            target,
            edge_type: RevsetGraphEdgeType::Direct,
        }
    }
    pub fn indirect(target: IndexPosition) -> Self {
        Self {
            target,
            edge_type: RevsetGraphEdgeType::Indirect,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum RevsetGraphEdgeType {
    Missing,
    Direct,
    Indirect,
}

// Given an iterator over some set of revisions, yields the same revisions with
// associated edge types.
//
// If a revision's parent is in the input set, then the edge will be "direct".
// Otherwise, there will be one "indirect" edge for each closest ancestor in the
// set, and one "missing" edge for each edge leading outside the set.
//
// Example (uppercase characters are in the input set):
//
// A          A
// |\         |\
// B c        B :
// |\|     => |\:
// d E        ~ E
// |/          ~
// root
//
// The implementation works by walking the input iterator in one commit at a
// time. It then considers all parents of the commit. It looks ahead in the
// input iterator far enough that all the parents will have been consumed if
// they are in the input (and puts them away so we can emit them later). If a
// parent of the current commit is not in the input set (i.e. it was not
// in the look-ahead), we walk these external commits until we end up back back
// in the input set. That walk may result in consuming more elements from the
// input iterator. In the example above, when we consider "A", we will initially
// look ahead to "B" and "c". When we consider edges from the external commit
// "c", we will further consume the input iterator to "E".
//
// Missing edges are those that don't lead back into the input set. If all edges
// from an external commit are missing, we consider the edge to that edge to
// also be missing. In the example above, that means that "B" will have a
// missing edge to "d" rather than to the root.
pub struct RevsetGraphIterator<'revset, 'repo> {
    input_set_iter: RevsetIterator<'revset, 'repo>,
    // Commits in the input set we had to take out of the iterator while walking external
    // edges. Does not necessarily include the commit we're currently about to emit.
    look_ahead: BTreeMap<IndexPosition, IndexEntry<'repo>>,
    // The last consumed position. This is always the smallest key in the look_ahead map, but it's
    // faster to keep a separate field for it.
    min_position: IndexPosition,
    // Edges for commits not in the input set.
    // TODO: Remove unneeded entries here as we go (that's why it's an ordered map)?
    external_commits: BTreeMap<IndexPosition, HashSet<RevsetGraphEdge>>,
}

impl<'revset, 'repo> RevsetGraphIterator<'revset, 'repo> {
    pub fn new(iter: RevsetIterator<'revset, 'repo>) -> RevsetGraphIterator<'revset, 'repo> {
        RevsetGraphIterator {
            input_set_iter: iter,
            look_ahead: Default::default(),
            min_position: IndexPosition::MAX,
            external_commits: Default::default(),
        }
    }

    fn next_index_entry(&mut self) -> Option<IndexEntry<'repo>> {
        if let Some((_, index_entry)) = self.look_ahead.pop_last() {
            return Some(index_entry);
        }
        self.input_set_iter.next()
    }

    fn edges_from_internal_commit(
        &mut self,
        index_entry: &IndexEntry<'repo>,
    ) -> HashSet<RevsetGraphEdge> {
        let mut edges = HashSet::new();
        for parent in index_entry.parents() {
            let parent_position = parent.position();
            self.consume_to(parent_position);
            if self.look_ahead.contains_key(&parent_position) {
                edges.insert(RevsetGraphEdge::direct(parent_position));
            } else {
                let parent_edges = self.edges_from_external_commit(parent);
                if parent_edges
                    .iter()
                    .all(|edge| edge.edge_type == RevsetGraphEdgeType::Missing)
                {
                    edges.insert(RevsetGraphEdge::missing(parent_position));
                } else {
                    edges.extend(parent_edges);
                }
            }
        }
        edges
    }

    fn edges_from_external_commit(
        &mut self,
        index_entry: IndexEntry<'repo>,
    ) -> HashSet<RevsetGraphEdge> {
        let position = index_entry.position();
        let mut stack = vec![index_entry];
        while let Some(entry) = stack.last() {
            let position = entry.position();
            let mut edges = HashSet::new();
            let mut parents_complete = true;
            for parent in entry.parents() {
                let parent_position = parent.position();
                self.consume_to(parent_position);
                if self.look_ahead.contains_key(&parent_position) {
                    // We have found a path back into the input set
                    edges.insert(RevsetGraphEdge::indirect(parent_position));
                } else if let Some(parent_edges) = self.external_commits.get(&parent_position) {
                    if parent_edges
                        .iter()
                        .all(|edge| edge.edge_type == RevsetGraphEdgeType::Missing)
                    {
                        edges.insert(RevsetGraphEdge::missing(parent_position));
                    } else {
                        edges.extend(parent_edges.iter().cloned());
                    }
                } else if parent_position < self.min_position {
                    // The parent is not in the input set
                    edges.insert(RevsetGraphEdge::missing(parent_position));
                } else {
                    // The parent is not in the input set but it's somewhere in the range
                    // where we have commits in the input set, so continue searching.
                    stack.push(parent);
                    parents_complete = false;
                }
            }
            if parents_complete {
                stack.pop().unwrap();
                self.external_commits.insert(position, edges);
            }
        }
        self.external_commits.get(&position).unwrap().clone()
    }

    fn consume_to(&mut self, pos: IndexPosition) {
        while pos < self.min_position {
            if let Some(next_entry) = self.input_set_iter.next() {
                let next_position = next_entry.position();
                self.look_ahead.insert(next_position, next_entry);
                self.min_position = next_position;
            } else {
                break;
            }
        }
    }
}

impl<'revset, 'repo> Iterator for RevsetGraphIterator<'revset, 'repo> {
    type Item = (IndexEntry<'repo>, HashSet<RevsetGraphEdge>);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(index_entry) = self.next_index_entry() {
            let edges = self.edges_from_internal_commit(&index_entry);
            Some((index_entry, edges))
        } else {
            None
        }
    }
}
