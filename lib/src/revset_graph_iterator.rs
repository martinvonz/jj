// Copyright 2021 The Jujutsu Authors
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

use std::cmp::min;
use std::collections::{BTreeMap, HashSet};

use crate::default_index_store::{IndexEntry, IndexPosition};
use crate::nightly_shims::BTreeMapExt;
use crate::revset::{Revset, RevsetGraphEdge, RevsetGraphEdgeType};

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
//
// The iterator can be configured to skip transitive edges that it would
// otherwise return. In this mode (which is the default), the edge from "A" to
// "E" in the example above would be excluded because there's also a transitive
// path from "A" to "E" via "B". The implementation of that mode
// adds a filtering step just before yielding the edges for a commit. The
// filtering works doing a DFS in the simplified graph. That may require even
// more look-ahead. Consider this example (uppercase characters are in the input
// set):
//
//   J
//  /|
// | i
// | |\
// | | H
// G | |
// | e f
// |  \|\
// |   D |
//  \ /  c
//   b  /
//   |/
//   A
//   |
//  root
//
// When walking from "J", we'll find indirect edges to "H", "G", and "D". This
// is our unfiltered set of edges, before removing transitive edges. In order to
// know that "D" is an ancestor of "H", we need to also walk from "H". We use
// the same search for finding edges from "H" as we used from "J". That results
// in looking ahead all the way to "A". We could reduce the amount of look-ahead
// by stopping at "c" since we're only interested in edges that could lead to
// "D", but that would require extra book-keeping to remember for later that the
// edges from "f" and "H" are only partially computed.
pub struct RevsetGraphIterator<'revset, 'index> {
    input_set_iter: Box<dyn Iterator<Item = IndexEntry<'index>> + 'revset>,
    // Commits in the input set we had to take out of the iterator while walking external
    // edges. Does not necessarily include the commit we're currently about to emit.
    look_ahead: BTreeMap<IndexPosition, IndexEntry<'index>>,
    // The last consumed position. This is always the smallest key in the look_ahead map, but it's
    // faster to keep a separate field for it.
    min_position: IndexPosition,
    // Edges for commits not in the input set.
    // TODO: Remove unneeded entries here as we go (that's why it's an ordered map)?
    edges: BTreeMap<IndexPosition, HashSet<RevsetGraphEdge>>,
    skip_transitive_edges: bool,
}

impl<'revset, 'index> RevsetGraphIterator<'revset, 'index> {
    pub fn new(revset: &'revset dyn Revset<'index>) -> RevsetGraphIterator<'revset, 'index> {
        RevsetGraphIterator {
            input_set_iter: revset.iter(),
            look_ahead: Default::default(),
            min_position: IndexPosition::MAX,
            edges: Default::default(),
            skip_transitive_edges: true,
        }
    }

    pub fn set_skip_transitive_edges(mut self, skip_transitive_edges: bool) -> Self {
        self.skip_transitive_edges = skip_transitive_edges;
        self
    }

    fn next_index_entry(&mut self) -> Option<IndexEntry<'index>> {
        if let Some(index_entry) = self.look_ahead.pop_last_value() {
            return Some(index_entry);
        }
        self.input_set_iter.next()
    }

    fn edges_from_internal_commit(
        &mut self,
        index_entry: &IndexEntry<'index>,
    ) -> HashSet<RevsetGraphEdge> {
        if let Some(edges) = self.edges.get(&index_entry.position()) {
            return edges.clone();
        }
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
        self.edges.insert(index_entry.position(), edges.clone());
        edges
    }

    fn edges_from_external_commit(
        &mut self,
        index_entry: IndexEntry<'index>,
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
                } else if let Some(parent_edges) = self.edges.get(&parent_position) {
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
                self.edges.insert(position, edges);
            }
        }
        self.edges.get(&position).unwrap().clone()
    }

    fn remove_transitive_edges(
        &mut self,
        edges: HashSet<RevsetGraphEdge>,
    ) -> HashSet<RevsetGraphEdge> {
        if !edges
            .iter()
            .any(|edge| edge.edge_type == RevsetGraphEdgeType::Indirect)
        {
            return edges;
        }
        let mut min_generation = u32::MAX;
        let mut initial_targets = HashSet::new();
        let mut work = vec![];
        // To start with, add the edges one step after the input edges.
        for edge in &edges {
            initial_targets.insert(edge.target);
            if edge.edge_type != RevsetGraphEdgeType::Missing {
                let entry = self.look_ahead.get(&edge.target).unwrap().clone();
                min_generation = min(min_generation, entry.generation_number());
                work.extend(self.edges_from_internal_commit(&entry));
            }
        }
        // Find commits reachable transitively and add them to the `unwanted` set.
        let mut unwanted = HashSet::new();
        while let Some(edge) = work.pop() {
            if edge.edge_type == RevsetGraphEdgeType::Missing || edge.target < self.min_position {
                continue;
            }
            if !unwanted.insert(edge.target) {
                // Already visited
                continue;
            }
            if initial_targets.contains(&edge.target) {
                // Already visited
                continue;
            }
            let entry = self.look_ahead.get(&edge.target).unwrap().clone();
            if entry.generation_number() < min_generation {
                continue;
            }
            work.extend(self.edges_from_internal_commit(&entry));
        }

        edges
            .into_iter()
            .filter(|edge| !unwanted.contains(&edge.target))
            .collect()
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

impl<'revset, 'index> Iterator for RevsetGraphIterator<'revset, 'index> {
    type Item = (IndexEntry<'index>, Vec<RevsetGraphEdge>);

    fn next(&mut self) -> Option<Self::Item> {
        let index_entry = self.next_index_entry()?;
        let mut edges = self.edges_from_internal_commit(&index_entry);
        if self.skip_transitive_edges {
            edges = self.remove_transitive_edges(edges);
        }
        let mut edges: Vec<_> = edges.into_iter().collect();
        edges.sort_by(|edge1, edge2| edge2.target.cmp(&edge1.target));
        Some((index_entry, edges))
    }
}
