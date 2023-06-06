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

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::hash::Hash;
use std::{iter, mem};

use itertools::Itertools as _;

pub fn dfs<T, ID, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> impl Iterator<Item = T>
where
    ID: Hash + Eq,
    II: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    let mut work: Vec<T> = start.into_iter().collect();
    let mut visited: HashSet<ID> = HashSet::new();
    iter::from_fn(move || loop {
        let c = work.pop()?;
        let id = id_fn(&c);
        if visited.contains(&id) {
            continue;
        }
        for p in neighbors_fn(&c) {
            work.push(p);
        }
        visited.insert(id);
        return Some(c);
    })
}

pub fn topo_order_forward<T, ID, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> Vec<T>
where
    ID: Hash + Eq + Clone,
    II: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    let mut stack = start.into_iter().map(|node| (node, false)).collect_vec();
    let mut visiting = HashSet::new();
    let mut emitted = HashSet::new();
    let mut result = vec![];
    while let Some((node, neighbors_visited)) = stack.pop() {
        let id = id_fn(&node);
        if emitted.contains(&id) {
            continue;
        }
        if !neighbors_visited {
            assert!(visiting.insert(id.clone()), "graph has cycle");
            let neighbors = neighbors_fn(&node);
            stack.push((node, true));
            stack.extend(neighbors.into_iter().map(|neighbor| (neighbor, false)));
        } else {
            visiting.remove(&id);
            emitted.insert(id);
            result.push(node);
        }
    }
    result
}

/// Returns neighbors before the node itself.
pub fn topo_order_reverse<T, ID, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    neighbors_fn: impl FnMut(&T) -> NI,
) -> Vec<T>
where
    ID: Hash + Eq + Clone,
    II: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    let mut result = topo_order_forward(start, id_fn, neighbors_fn);
    result.reverse();
    result
}

/// Like `topo_order_reverse()`, but can iterate linear DAG lazily.
///
/// The DAG is supposed to be (mostly) topologically ordered by `T: Ord`.
/// For example, topological order of chronological data should respect
/// timestamp (except a few outliers caused by clock skew.)
///
/// Use `topo_order_reverse()` if the DAG is heavily branched. This can
/// only process linear part lazily.
pub fn topo_order_reverse_lazy<T, ID, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> impl Iterator<Item = T>
where
    T: Ord,
    ID: Hash + Eq + Clone,
    II: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    let mut inner = TopoOrderReverseLazyInner::new(start.into_iter().collect());
    iter::from_fn(move || inner.next(&id_fn, &mut neighbors_fn))
}

#[derive(Clone, Debug)]
struct TopoOrderReverseLazyInner<T, ID> {
    start: Vec<T>,
    result: Vec<T>,
    emitted: HashSet<ID>,
}

impl<T: Ord, ID: Hash + Eq + Clone> TopoOrderReverseLazyInner<T, ID> {
    fn new(start: Vec<T>) -> Self {
        TopoOrderReverseLazyInner {
            start,
            result: Vec::new(),
            emitted: HashSet::new(),
        }
    }

    fn next<NI: IntoIterator<Item = T>>(
        &mut self,
        id_fn: impl Fn(&T) -> ID,
        mut neighbors_fn: impl FnMut(&T) -> NI,
    ) -> Option<T> {
        if let Some(node) = self.result.pop() {
            return Some(node);
        }

        // Fast path for linear DAG
        if self.start.len() <= 1 {
            let node = self.start.pop()?;
            self.start.extend(neighbors_fn(&node));
            assert!(self.emitted.insert(id_fn(&node)), "graph has cycle");
            return Some(node);
        }

        // Extract graph nodes based on T's order, and sort them by using ids
        // (because we wouldn't want to clone T itself)
        let start_ids = self.start.iter().map(&id_fn).collect_vec();
        let (mut node_map, neighbor_ids_map, remainder) =
            look_ahead_sub_graph(mem::take(&mut self.start), &id_fn, &mut neighbors_fn);
        self.start = remainder;
        let sorted_ids = topo_order_forward(&start_ids, |id| *id, |id| &neighbor_ids_map[id]);
        self.result.reserve(sorted_ids.len());
        for id in sorted_ids {
            let (id, node) = node_map.remove_entry(id).unwrap();
            assert!(self.emitted.insert(id), "graph has cycle");
            self.result.push(node);
        }
        self.result.pop()
    }
}

/// Splits DAG at single fork point, and extracts branchy part as sub graph.
///
/// ```text
///  o | C
///  | o B
///  |/ <---- split here (A->B or A->C would create cycle)
///  o A
/// ```
///
/// If a branch reached to root (empty neighbors), the graph can't be split
/// anymore because the other branch may be connected to a descendant of
/// the rooted branch.
///
/// ```text
///  o | C
///  | o B
///  |  <---- can't split here (there may be edge A->B)
///  o A
/// ```
///
/// We assume the graph is (mostly) topologically ordered by `T: Ord`.
#[allow(clippy::type_complexity)]
fn look_ahead_sub_graph<T, ID, NI>(
    start: Vec<T>,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> (HashMap<ID, T>, HashMap<ID, Vec<ID>>, Vec<T>)
where
    T: Ord,
    ID: Hash + Eq + Clone,
    NI: IntoIterator<Item = T>,
{
    let mut queue: BinaryHeap<T> = start.into();
    // Build separate node/neighbors maps since lifetime is different at caller
    let mut node_map: HashMap<ID, T> = HashMap::new();
    let mut neighbor_ids_map: HashMap<ID, Vec<ID>> = HashMap::new();
    let mut has_reached_root = false;
    while queue.len() > 1 || node_map.is_empty() || has_reached_root {
        let node = if let Some(node) = queue.pop() {
            node
        } else {
            break;
        };
        let node_id = id_fn(&node);
        if node_map.contains_key(&node_id) {
            continue;
        }

        let mut neighbor_ids = Vec::new();
        let mut neighbors_iter = neighbors_fn(&node).into_iter().peekable();
        has_reached_root |= neighbors_iter.peek().is_none();
        for neighbor in neighbors_iter {
            neighbor_ids.push(id_fn(&neighbor));
            queue.push(neighbor);
        }
        node_map.insert(node_id.clone(), node);
        neighbor_ids_map.insert(node_id, neighbor_ids);
    }

    assert!(queue.len() <= 1, "order of remainder shouldn't matter");
    let remainder = queue.into_vec();

    // Omit unvisited neighbors
    if let Some(unvisited_id) = remainder.first().map(&id_fn) {
        for neighbor_ids in neighbor_ids_map.values_mut() {
            neighbor_ids.retain(|id| *id != unvisited_id);
        }
    }

    (node_map, neighbor_ids_map, remainder)
}

pub fn leaves<T, ID, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
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
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> HashSet<T>
where
    T: Hash + Eq + Clone,
    ID: Hash + Eq,
    II: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    let start: Vec<T> = start.into_iter().collect();
    let mut reachable: HashSet<T> = start.iter().cloned().collect();
    for _node in dfs(start.into_iter(), id_fn, |node| {
        let neighbors: Vec<T> = neighbors_fn(node).into_iter().collect();
        for neighbor in &neighbors {
            reachable.remove(neighbor);
        }
        neighbors
    }) {}
    reachable
}

pub fn closest_common_node<T, ID, II1, II2, NI>(
    set1: II1,
    set2: II2,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> Option<T>
where
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
    use std::panic;

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
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let common = topo_order_reverse(vec!['C'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['C', 'B', 'A']);
        let common = topo_order_reverse(vec!['C', 'B'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['C', 'B', 'A']);
        let common = topo_order_reverse(vec!['B', 'C'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['C', 'B', 'A']);

        let common = topo_order_reverse_lazy(vec!['C'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['C', 'B', 'A']);
        let common = topo_order_reverse_lazy(vec!['C', 'B'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['C', 'B', 'A']);
        let common = topo_order_reverse_lazy(vec!['B', 'C'], id_fn, neighbors_fn).collect_vec();
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
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let common = topo_order_reverse(vec!['F'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['F', 'E', 'D', 'C', 'B', 'A']);
        let common = topo_order_reverse(vec!['F', 'E', 'C'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['F', 'D', 'E', 'C', 'B', 'A']);
        let common = topo_order_reverse(vec!['F', 'D', 'E'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['F', 'D', 'C', 'B', 'E', 'A']);

        let common = topo_order_reverse_lazy(vec!['F'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['F', 'E', 'D', 'C', 'B', 'A']);
        let common =
            topo_order_reverse_lazy(vec!['F', 'E', 'C'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['F', 'D', 'E', 'C', 'B', 'A']);
        let common =
            topo_order_reverse_lazy(vec!['F', 'D', 'E'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['F', 'D', 'C', 'B', 'E', 'A']);
    }

    #[test]
    fn test_topo_order_reverse_nested_merges() {
        // This graph:
        //  o I
        //  |\
        //  | o H
        //  | |\
        //  | | o G
        //  | o | F
        //  | | o E
        //  o |/ D
        //  | o C
        //  o | B
        //  |/
        //  o A

        let neighbors = hashmap! {
            'A' => vec![],
            'B' => vec!['A'],
            'C' => vec!['A'],
            'D' => vec!['B'],
            'E' => vec!['C'],
            'F' => vec!['C'],
            'G' => vec!['E'],
            'H' => vec!['F', 'G'],
            'I' => vec!['D', 'H'],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let common = topo_order_reverse(vec!['I'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['I', 'D', 'B', 'H', 'F', 'G', 'E', 'C', 'A']);

        let common = topo_order_reverse_lazy(vec!['I'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['I', 'D', 'B', 'H', 'F', 'G', 'E', 'C', 'A']);
    }

    #[test]
    fn test_topo_order_reverse_nested_merges_bad_order() {
        // This graph:
        //  o I
        //  |\
        //  | |\
        //  | | |\
        //  | | | o h (h > I)
        //  | | |/|
        //  | | o | G
        //  | |/| o f
        //  | o |/ e (e > I, G)
        //  |/| o D
        //  o |/ C
        //  | o b (b > D)
        //  |/
        //  o A

        let neighbors = hashmap! {
            'A' => vec![],
            'b' => vec!['A'],
            'C' => vec!['A'],
            'D' => vec!['b'],
            'e' => vec!['C', 'b'],
            'f' => vec!['D'],
            'G' => vec!['e', 'D'],
            'h' => vec!['G', 'f'],
            'I' => vec!['C', 'e', 'G', 'h'],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let common = topo_order_reverse(vec!['I'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['I', 'h', 'G', 'e', 'C', 'f', 'D', 'b', 'A']);

        let common = topo_order_reverse_lazy(vec!['I'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['I', 'h', 'G', 'e', 'C', 'f', 'D', 'b', 'A']);
    }

    #[test]
    fn test_topo_order_reverse_merge_bad_fork_order_at_root() {
        // This graph:
        //  o E
        //  |\
        //  o | D
        //  | o C
        //  | o B
        //  |/
        //  o a (a > D, B)

        let neighbors = hashmap! {
            'a' => vec![],
            'B' => vec!['a'],
            'C' => vec!['B'],
            'D' => vec!['a'],
            'E' => vec!['D', 'C'],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let common = topo_order_reverse(vec!['E'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['E', 'D', 'C', 'B', 'a']);

        // The root node 'a' is visited before 'C'. If the graph were split there,
        // the branch 'C->B->a' would be orphaned.
        let common = topo_order_reverse_lazy(vec!['E'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['E', 'D', 'C', 'B', 'a']);
    }

    #[test]
    fn test_topo_order_reverse_merge_and_linear() {
        // This graph:
        //  o G
        //  |\
        //  | o F
        //  o | E
        //  | o D
        //  |/
        //  o C
        //  o B
        //  o A

        let neighbors = hashmap! {
            'A' => vec![],
            'B' => vec!['A'],
            'C' => vec!['B'],
            'D' => vec!['C'],
            'E' => vec!['C'],
            'F' => vec!['D'],
            'G' => vec!['E', 'F'],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let common = topo_order_reverse(vec!['G'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['G', 'E', 'F', 'D', 'C', 'B', 'A']);

        let common = topo_order_reverse_lazy(vec!['G'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['G', 'E', 'F', 'D', 'C', 'B', 'A']);

        // Iterator can be lazy for linear chunks.
        let mut inner_iter = TopoOrderReverseLazyInner::new(vec!['G']);
        assert_eq!(inner_iter.next(id_fn, neighbors_fn), Some('G'));
        assert!(!inner_iter.start.is_empty());
        assert!(inner_iter.result.is_empty());
        assert_eq!(
            iter::from_fn(|| inner_iter.next(id_fn, neighbors_fn))
                .take(4)
                .collect_vec(),
            vec!['E', 'F', 'D', 'C'],
        );
        assert!(!inner_iter.start.is_empty());
        assert!(inner_iter.result.is_empty());
    }

    #[test]
    fn test_topo_order_reverse_merge_and_linear_bad_fork_order() {
        // This graph:
        //  o G
        //  |\
        //  o | F
        //  o | E
        //  | o D
        //  |/
        //  o c (c > E, D)
        //  o B
        //  o A

        let neighbors = hashmap! {
            'A' => vec![],
            'B' => vec!['A'],
            'c' => vec!['B'],
            'D' => vec!['c'],
            'E' => vec!['c'],
            'F' => vec!['E'],
            'G' => vec!['F', 'D'],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let common = topo_order_reverse(vec!['G'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['G', 'F', 'E', 'D', 'c', 'B', 'A']);

        let common = topo_order_reverse_lazy(vec!['G'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['G', 'F', 'E', 'D', 'c', 'B', 'A']);

        // Iterator can be lazy for linear chunks. The node 'c' is visited before 'D',
        // but it will be processed lazily.
        let mut inner_iter = TopoOrderReverseLazyInner::new(vec!['G']);
        assert_eq!(inner_iter.next(id_fn, neighbors_fn), Some('G'));
        assert!(!inner_iter.start.is_empty());
        assert!(inner_iter.result.is_empty());
        assert_eq!(
            iter::from_fn(|| inner_iter.next(id_fn, neighbors_fn))
                .take(4)
                .collect_vec(),
            vec!['F', 'E', 'D', 'c'],
        );
        assert!(!inner_iter.start.is_empty());
        assert!(inner_iter.result.is_empty());
    }

    #[test]
    fn test_topo_order_reverse_merge_and_linear_bad_merge_order() {
        // This graph:
        //  o G
        //  |\
        //  o | f (f > G)
        //  o | e
        //  | o d (d > G)
        //  |/
        //  o C
        //  o B
        //  o A

        let neighbors = hashmap! {
            'A' => vec![],
            'B' => vec!['A'],
            'C' => vec!['B'],
            'd' => vec!['C'],
            'e' => vec!['C'],
            'f' => vec!['e'],
            'G' => vec!['f', 'd'],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let common = topo_order_reverse(vec!['G'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['G', 'f', 'e', 'd', 'C', 'B', 'A']);

        let common = topo_order_reverse_lazy(vec!['G'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['G', 'f', 'e', 'd', 'C', 'B', 'A']);

        // Iterator can be lazy for linear chunks.
        let mut inner_iter = TopoOrderReverseLazyInner::new(vec!['G']);
        assert_eq!(inner_iter.next(id_fn, neighbors_fn), Some('G'));
        assert!(!inner_iter.start.is_empty());
        assert!(inner_iter.result.is_empty());
        assert_eq!(
            iter::from_fn(|| inner_iter.next(id_fn, neighbors_fn))
                .take(4)
                .collect_vec(),
            vec!['f', 'e', 'd', 'C'],
        );
        assert!(!inner_iter.start.is_empty());
        assert!(inner_iter.result.is_empty());
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
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let common = topo_order_reverse(vec!['F', 'C'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['F', 'E', 'D', 'C', 'B', 'A']);

        let common = topo_order_reverse_lazy(vec!['F', 'C'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['F', 'E', 'D', 'C', 'B', 'A']);
    }

    #[test]
    fn test_topo_order_reverse_multiple_roots() {
        // This graph:
        //  o D
        //  | \
        //  o | C
        //    o B
        //    o A

        let neighbors = hashmap! {
            'A' => vec![],
            'B' => vec!['A'],
            'C' => vec![],
            'D' => vec!['C', 'B'],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let common = topo_order_reverse(vec!['D'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['D', 'C', 'B', 'A']);

        let common = topo_order_reverse_lazy(vec!['D'], id_fn, neighbors_fn).collect_vec();
        assert_eq!(common, vec!['D', 'C', 'B', 'A']);
    }

    #[test]
    fn test_topo_order_reverse_cycle_linear() {
        // This graph:
        //  o C
        //  o B
        //  o A (to C)

        let neighbors = hashmap! {
            'A' => vec!['C'],
            'B' => vec!['A'],
            'C' => vec!['B'],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let result = panic::catch_unwind(|| topo_order_reverse(vec!['C'], id_fn, neighbors_fn));
        assert!(result.is_err());

        topo_order_reverse_lazy(vec!['C'], id_fn, neighbors_fn)
            .take(3)
            .collect_vec(); // sanity check
        let result = panic::catch_unwind(|| {
            topo_order_reverse_lazy(vec!['C'], id_fn, neighbors_fn)
                .take(4)
                .collect_vec()
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_topo_order_reverse_cycle_to_branchy_sub_graph() {
        // This graph:
        //  o D
        //  |\
        //  | o C
        //  |/
        //  o B
        //  o A (to C)

        let neighbors = hashmap! {
            'A' => vec!['C'],
            'B' => vec!['A'],
            'C' => vec!['B'],
            'D' => vec!['B', 'C'],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let result = panic::catch_unwind(|| topo_order_reverse(vec!['D'], id_fn, neighbors_fn));
        assert!(result.is_err());

        topo_order_reverse_lazy(vec!['D'], id_fn, neighbors_fn)
            .take(4)
            .collect_vec(); // sanity check
        let result = panic::catch_unwind(|| {
            topo_order_reverse_lazy(vec!['D'], id_fn, neighbors_fn)
                .take(5)
                .collect_vec()
        });
        assert!(result.is_err());
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
            |node| *node,
            |node| neighbors[node].clone(),
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
            |node| *node,
            |node| neighbors[node].clone(),
        );
        assert_eq!(actual, hashset!['D', 'F']);

        // Check with a different order in the start set
        let actual = heads(
            vec!['F', 'D', 'C', 'A'],
            |node| *node,
            |node| neighbors[node].clone(),
        );
        assert_eq!(actual, hashset!['D', 'F']);
    }
}
