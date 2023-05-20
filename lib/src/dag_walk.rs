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

use std::collections::HashSet;
use std::hash::Hash;
use std::iter::Iterator;

pub struct BfsIter<'id_fn, 'neighbors_fn, T, ID, NI> {
    id_fn: Box<dyn Fn(&T) -> ID + 'id_fn>,
    neighbors_fn: Box<dyn FnMut(&T) -> NI + 'neighbors_fn>,
    work: Vec<T>,
    visited: HashSet<ID>,
}

impl<T, ID, NI> Iterator for BfsIter<'_, '_, T, ID, NI>
where
    ID: Hash + Eq,
    NI: IntoIterator<Item = T>,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let c = self.work.pop()?;
            let id = (self.id_fn)(&c);
            if self.visited.contains(&id) {
                continue;
            }
            for p in (self.neighbors_fn)(&c) {
                self.work.push(p);
            }
            self.visited.insert(id);
            return Some(c);
        }
    }
}

pub fn bfs<'id_fn, 'neighbors_fn, T, ID, II, NI>(
    start: II,
    id_fn: Box<dyn Fn(&T) -> ID + 'id_fn>,
    neighbors_fn: Box<dyn FnMut(&T) -> NI + 'neighbors_fn>,
) -> BfsIter<'id_fn, 'neighbors_fn, T, ID, NI>
where
    ID: Hash + Eq,
    II: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    BfsIter {
        id_fn,
        neighbors_fn,
        work: start.into_iter().collect(),
        visited: Default::default(),
    }
}

/// Returns neighbors before the node itself.
pub fn topo_order_reverse<'a, T, ID, II, NI>(
    start: II,
    id_fn: Box<dyn Fn(&T) -> ID + 'a>,
    mut neighbors_fn: Box<dyn FnMut(&T) -> NI + 'a>,
) -> Vec<T>
where
    T: Hash + Eq + Clone,
    ID: Hash + Eq + Clone,
    II: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    let mut visiting = HashSet::new();
    let mut emitted = HashSet::new();
    let mut result = vec![];

    let mut start_nodes: Vec<T> = start.into_iter().collect();
    start_nodes.reverse();

    for start_node in start_nodes {
        let mut stack = vec![(start_node, false)];
        while let Some((node, neighbors_visited)) = stack.pop() {
            let id = id_fn(&node);
            if emitted.contains(&id) {
                continue;
            }
            if !neighbors_visited {
                assert!(visiting.insert(id.clone()), "graph has cycle");
                let neighbors = neighbors_fn(&node);
                stack.push((node, true));
                for neighbor in neighbors {
                    stack.push((neighbor, false));
                }
            } else {
                visiting.remove(&id);
                emitted.insert(id);
                result.push(node);
            }
        }
    }
    result.reverse();
    result
}

pub fn leaves<T, ID, II, NI>(
    start: II,
    mut neighbors_fn: impl FnMut(&T) -> NI,
    id_fn: impl Fn(&T) -> ID,
) -> HashSet<T>
where
    T: Hash + Eq + Clone,
    ID: Hash + Eq,
    II: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    let mut visited = HashSet::new();
    let mut work: Vec<T> = start.into_iter().collect();
    let mut leaves: HashSet<T> = work.iter().cloned().collect();
    let mut non_leaves = HashSet::new();
    while !work.is_empty() {
        // TODO: make this not waste so much memory on the sets
        let mut new_work = vec![];
        for c in work {
            let id: ID = id_fn(&c);
            if visited.contains(&id) {
                continue;
            }
            for p in neighbors_fn(&c) {
                non_leaves.insert(c.clone());
                new_work.push(p);
            }
            visited.insert(id);
            leaves.insert(c);
        }
        work = new_work;
    }
    leaves.difference(&non_leaves).cloned().collect()
}

/// Find nodes in the start set that are not reachable from other nodes in the
/// start set.
pub fn heads<T, ID, II, NI>(
    start: II,
    neighbors_fn: impl Fn(&T) -> NI,
    id_fn: impl Fn(&T) -> ID,
) -> HashSet<T>
where
    T: Hash + Eq + Clone,
    ID: Hash + Eq,
    II: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    let start: Vec<T> = start.into_iter().collect();
    let mut reachable: HashSet<T> = start.iter().cloned().collect();
    for _node in bfs(
        start.into_iter(),
        Box::new(id_fn),
        Box::new(|node| {
            let neighbors: Vec<T> = neighbors_fn(node).into_iter().collect();
            for neighbor in &neighbors {
                reachable.remove(neighbor);
            }
            neighbors
        }),
    ) {}
    reachable
}

pub fn closest_common_node<T, ID, II1, II2, NI>(
    set1: II1,
    set2: II2,
    neighbors_fn: impl Fn(&T) -> NI,
    id_fn: impl Fn(&T) -> ID,
) -> Option<T>
where
    T: Hash + Eq + Clone,
    ID: Hash + Eq,
    II1: IntoIterator<Item = T>,
    II2: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    let mut visited1 = HashSet::new();
    let mut visited2 = HashSet::new();

    let mut work1: Vec<T> = set1.into_iter().collect();
    let mut work2: Vec<T> = set2.into_iter().collect();
    while !work1.is_empty() || !work2.is_empty() {
        let mut new_work1 = vec![];
        for node in work1 {
            let id: ID = id_fn(&node);
            if visited2.contains(&id) {
                return Some(node);
            }
            if visited1.insert(id) {
                for neighbor in neighbors_fn(&node) {
                    new_work1.push(neighbor);
                }
            }
        }
        work1 = new_work1;

        let mut new_work2 = vec![];
        for node in work2 {
            let id: ID = id_fn(&node);
            if visited1.contains(&id) {
                return Some(node);
            }
            if visited2.insert(id) {
                for neighbor in neighbors_fn(&node) {
                    new_work2.push(neighbor);
                }
            }
        }
        work2 = new_work2;
    }
    None
}

#[cfg(test)]
mod tests {
    use maplit::{hashmap, hashset};

    use super::*;

    #[test]
    fn test_topo_order_reverse_linear() {
        // This graph:
        //  o C
        //  o B
        //  o A

        let neighbors = hashmap! {
            'A' => vec![],
            'B' => vec!['A'],
            'C' => vec!['B'],
        };

        let common = topo_order_reverse(
            vec!['C'],
            Box::new(|node| *node),
            Box::new(move |node| neighbors[node].clone()),
        );

        assert_eq!(common, vec!['C', 'B', 'A']);
    }

    #[test]
    fn test_topo_order_reverse_merge() {
        // This graph:
        //  o F
        //  |\
        //  o | E
        //  | o D
        //  | o C
        //  | o B
        //  |/
        //  o A

        let neighbors = hashmap! {
            'A' => vec![],
            'B' => vec!['A'],
            'C' => vec!['B'],
            'D' => vec!['C'],
            'E' => vec!['A'],
            'F' => vec!['E', 'D'],
        };

        let common = topo_order_reverse(
            vec!['F'],
            Box::new(|node| *node),
            Box::new(move |node| neighbors[node].clone()),
        );

        assert_eq!(common, vec!['F', 'E', 'D', 'C', 'B', 'A']);
    }

    #[test]
    fn test_topo_order_reverse_multiple_heads() {
        // This graph:
        //  o F
        //  |\
        //  o | E
        //  | o D
        //  | | o C
        //  | | |
        //  | | o B
        //  | |/
        //  |/
        //  o A

        let neighbors = hashmap! {
            'A' => vec![],
            'B' => vec!['A'],
            'C' => vec!['B'],
            'D' => vec!['A'],
            'E' => vec!['A'],
            'F' => vec!['E', 'D'],
        };

        let common = topo_order_reverse(
            vec!['F', 'C'],
            Box::new(|node| *node),
            Box::new(move |node| neighbors[node].clone()),
        );

        assert_eq!(common, vec!['F', 'E', 'D', 'C', 'B', 'A']);
    }

    #[test]
    fn test_closest_common_node_tricky() {
        // Test this case where A is the shortest distance away, but we still want the
        // result to be B because A is an ancestor of B. In other words, we want
        // to minimize the longest distance.
        //
        //  E       H
        //  |\     /|
        //  | D   G |
        //  | C   F |
        //   \ \ / /
        //    \ B /
        //     \|/
        //      A

        let neighbors = hashmap! {
            'A' => vec![],
            'B' => vec!['A'],
            'C' => vec!['B'],
            'D' => vec!['C'],
            'E' => vec!['A','D'],
            'F' => vec!['B'],
            'G' => vec!['F'],
            'H' => vec!['A', 'G'],
        };

        let common = closest_common_node(
            vec!['E'],
            vec!['H'],
            |node| neighbors[node].clone(),
            |node| *node,
        );

        // TODO: fix the implementation to return B
        assert_eq!(common, Some('A'));
    }

    #[test]
    fn test_heads_mixed() {
        // Test the uppercase letters are in the start set
        //
        //  D F
        //  |/|
        //  C e
        //  |/
        //  b
        //  |
        //  A

        let neighbors = hashmap! {
            'A' => vec![],
            'b' => vec!['A'],
            'C' => vec!['b'],
            'D' => vec!['C'],
            'e' => vec!['b'],
            'F' => vec!['C', 'e'],
        };

        let actual = heads(
            vec!['A', 'C', 'D', 'F'],
            |node| neighbors[node].clone(),
            |node| *node,
        );
        assert_eq!(actual, hashset!['D', 'F']);

        // Check with a different order in the start set
        let actual = heads(
            vec!['F', 'D', 'C', 'A'],
            |node| neighbors[node].clone(),
            |node| *node,
        );
        assert_eq!(actual, hashset!['D', 'F']);
    }
}
