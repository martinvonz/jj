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

//! General-purpose DAG algorithms.

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::convert::Infallible;
use std::hash::Hash;
use std::{iter, mem};

use itertools::Itertools as _;

/// Traverses nodes from `start` in depth-first order.
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
    let neighbors_fn = move |node: &T| to_ok_iter(neighbors_fn(node));
    dfs_ok(to_ok_iter(start), id_fn, neighbors_fn).map(Result::unwrap)
}

/// Traverses nodes from `start` in depth-first order.
///
/// An `Err` is emitted as a node with no neighbors. Caller may decide to
/// short-circuit on it.
pub fn dfs_ok<T, ID, E, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> impl Iterator<Item = Result<T, E>>
where
    ID: Hash + Eq,
    II: IntoIterator<Item = Result<T, E>>,
    NI: IntoIterator<Item = Result<T, E>>,
{
    let mut work: Vec<Result<T, E>> = start.into_iter().collect();
    let mut visited: HashSet<ID> = HashSet::new();
    iter::from_fn(move || loop {
        let c = match work.pop() {
            Some(Ok(c)) => c,
            r @ (Some(Err(_)) | None) => return r,
        };
        let id = id_fn(&c);
        if visited.contains(&id) {
            continue;
        }
        for p in neighbors_fn(&c) {
            work.push(p);
        }
        visited.insert(id);
        return Some(Ok(c));
    })
}

/// Builds a list of nodes reachable from the `start` where neighbors come
/// before the node itself.
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
    let neighbors_fn = move |node: &T| to_ok_iter(neighbors_fn(node));
    topo_order_forward_ok(to_ok_iter(start), id_fn, neighbors_fn).unwrap()
}

/// Builds a list of `Ok` nodes reachable from the `start` where neighbors come
/// before the node itself.
///
/// If `start` or `neighbors_fn()` yields an `Err`, this function terminates and
/// returns the error.
pub fn topo_order_forward_ok<T, ID, E, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> Result<Vec<T>, E>
where
    ID: Hash + Eq + Clone,
    II: IntoIterator<Item = Result<T, E>>,
    NI: IntoIterator<Item = Result<T, E>>,
{
    let mut stack: Vec<(T, bool)> = start.into_iter().map(|r| Ok((r?, false))).try_collect()?;
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
            let neighbors_iter = neighbors_fn(&node).into_iter();
            stack.reserve(neighbors_iter.size_hint().0 + 1);
            stack.push((node, true));
            for neighbor in neighbors_iter {
                stack.push((neighbor?, false));
            }
        } else {
            visiting.remove(&id);
            emitted.insert(id);
            result.push(node);
        }
    }
    Ok(result)
}

/// Builds a list of nodes reachable from the `start` where neighbors come after
/// the node itself.
pub fn topo_order_reverse<T, ID, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> Vec<T>
where
    ID: Hash + Eq + Clone,
    II: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    let neighbors_fn = move |node: &T| to_ok_iter(neighbors_fn(node));
    topo_order_reverse_ok(to_ok_iter(start), id_fn, neighbors_fn).unwrap()
}

/// Builds a list of `Ok` nodes reachable from the `start` where neighbors come
/// after the node itself.
///
/// If `start` or `neighbors_fn()` yields an `Err`, this function terminates and
/// returns the error.
pub fn topo_order_reverse_ok<T, ID, E, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    neighbors_fn: impl FnMut(&T) -> NI,
) -> Result<Vec<T>, E>
where
    ID: Hash + Eq + Clone,
    II: IntoIterator<Item = Result<T, E>>,
    NI: IntoIterator<Item = Result<T, E>>,
{
    let mut result = topo_order_forward_ok(start, id_fn, neighbors_fn)?;
    result.reverse();
    Ok(result)
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
    let neighbors_fn = move |node: &T| to_ok_iter(neighbors_fn(node));
    topo_order_reverse_lazy_ok(to_ok_iter(start), id_fn, neighbors_fn).map(Result::unwrap)
}

/// Like `topo_order_reverse_ok()`, but can iterate linear DAG lazily.
///
/// The returned iterator short-circuits at an `Err`. Pending non-linear nodes
/// before the `Err` will be discarded.
pub fn topo_order_reverse_lazy_ok<T, ID, E, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> impl Iterator<Item = Result<T, E>>
where
    T: Ord,
    ID: Hash + Eq + Clone,
    II: IntoIterator<Item = Result<T, E>>,
    NI: IntoIterator<Item = Result<T, E>>,
{
    let mut inner = TopoOrderReverseLazyInner::empty();
    inner.extend(start);
    iter::from_fn(move || inner.next(&id_fn, &mut neighbors_fn))
}

#[derive(Clone, Debug)]
struct TopoOrderReverseLazyInner<T, ID, E> {
    start: Vec<T>,
    result: Vec<Result<T, E>>,
    emitted: HashSet<ID>,
}

impl<T: Ord, ID: Hash + Eq + Clone, E> TopoOrderReverseLazyInner<T, ID, E> {
    fn empty() -> Self {
        TopoOrderReverseLazyInner {
            start: Vec::new(),
            result: Vec::new(),
            emitted: HashSet::new(),
        }
    }

    fn extend(&mut self, iter: impl IntoIterator<Item = Result<T, E>>) {
        let iter = iter.into_iter();
        self.start.reserve(iter.size_hint().0);
        for res in iter {
            if let Ok(node) = res {
                self.start.push(node);
            } else {
                // Emit the error and terminate
                self.start.clear();
                self.result.insert(0, res);
                return;
            }
        }
    }

    fn next<NI: IntoIterator<Item = Result<T, E>>>(
        &mut self,
        id_fn: impl Fn(&T) -> ID,
        mut neighbors_fn: impl FnMut(&T) -> NI,
    ) -> Option<Result<T, E>> {
        if let Some(res) = self.result.pop() {
            return Some(res);
        }

        // Fast path for linear DAG
        if self.start.len() <= 1 {
            let node = self.start.pop()?;
            self.extend(neighbors_fn(&node));
            assert!(self.emitted.insert(id_fn(&node)), "graph has cycle");
            return Some(Ok(node));
        }

        // Extract graph nodes based on T's order, and sort them by using ids
        // (because we wouldn't want to clone T itself)
        let start_ids = self.start.iter().map(&id_fn).collect_vec();
        match look_ahead_sub_graph(mem::take(&mut self.start), &id_fn, &mut neighbors_fn) {
            Ok((mut node_map, neighbor_ids_map, remainder)) => {
                self.start = remainder;
                let sorted_ids =
                    topo_order_forward(&start_ids, |id| *id, |id| &neighbor_ids_map[id]);
                self.result.reserve(sorted_ids.len());
                for id in sorted_ids {
                    let (id, node) = node_map.remove_entry(id).unwrap();
                    assert!(self.emitted.insert(id), "graph has cycle");
                    self.result.push(Ok(node));
                }
                self.result.pop()
            }
            Err(err) => Some(Err(err)),
        }
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
fn look_ahead_sub_graph<T, ID, E, NI>(
    start: Vec<T>,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> Result<(HashMap<ID, T>, HashMap<ID, Vec<ID>>, Vec<T>), E>
where
    T: Ord,
    ID: Hash + Eq + Clone,
    NI: IntoIterator<Item = Result<T, E>>,
{
    let mut queue: BinaryHeap<T> = start.into();
    // Build separate node/neighbors maps since lifetime is different at caller
    let mut node_map: HashMap<ID, T> = HashMap::new();
    let mut neighbor_ids_map: HashMap<ID, Vec<ID>> = HashMap::new();
    let mut has_reached_root = false;
    while queue.len() > 1 || node_map.is_empty() || has_reached_root {
        let Some(node) = queue.pop() else {
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
            let neighbor = neighbor?;
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

    Ok((node_map, neighbor_ids_map, remainder))
}

/// Builds a list of nodes reachable from the `start` where neighbors come after
/// the node itself.
///
/// Unlike `topo_order_reverse()`, nodes are sorted in reverse `T: Ord` order so
/// long as they can respect the topological requirement.
pub fn topo_order_reverse_ord<T, ID, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> Vec<T>
where
    T: Ord,
    ID: Hash + Eq + Clone,
    II: IntoIterator<Item = T>,
    NI: IntoIterator<Item = T>,
{
    let neighbors_fn = move |node: &T| to_ok_iter(neighbors_fn(node));
    topo_order_reverse_ord_ok(to_ok_iter(start), id_fn, neighbors_fn).unwrap()
}

/// Builds a list of `Ok` nodes reachable from the `start` where neighbors come
/// after the node itself.
///
/// Unlike `topo_order_reverse_ok()`, nodes are sorted in reverse `T: Ord` order
/// so long as they can respect the topological requirement.
///
/// If `start` or `neighbors_fn()` yields an `Err`, this function terminates and
/// returns the error.
pub fn topo_order_reverse_ord_ok<T, ID, E, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> Result<Vec<T>, E>
where
    T: Ord,
    ID: Hash + Eq + Clone,
    II: IntoIterator<Item = Result<T, E>>,
    NI: IntoIterator<Item = Result<T, E>>,
{
    struct InnerNode<T> {
        node: Option<T>,
        indegree: usize,
    }

    // DFS to accumulate incoming edges
    let mut stack: Vec<T> = start.into_iter().try_collect()?;
    let mut head_node_map: HashMap<ID, T> = HashMap::new();
    let mut inner_node_map: HashMap<ID, InnerNode<T>> = HashMap::new();
    let mut neighbor_ids_map: HashMap<ID, Vec<ID>> = HashMap::new();
    while let Some(node) = stack.pop() {
        let node_id = id_fn(&node);
        if neighbor_ids_map.contains_key(&node_id) {
            continue; // Already visited
        }

        let neighbors_iter = neighbors_fn(&node).into_iter();
        let pos = stack.len();
        stack.reserve(neighbors_iter.size_hint().0);
        for neighbor in neighbors_iter {
            stack.push(neighbor?);
        }
        let neighbor_ids = stack[pos..].iter().map(&id_fn).collect_vec();
        if let Some(inner) = inner_node_map.get_mut(&node_id) {
            inner.node = Some(node);
        } else {
            head_node_map.insert(node_id.clone(), node);
        }

        for neighbor_id in &neighbor_ids {
            if let Some(inner) = inner_node_map.get_mut(neighbor_id) {
                inner.indegree += 1;
            } else {
                let inner = InnerNode {
                    node: head_node_map.remove(neighbor_id),
                    indegree: 1,
                };
                inner_node_map.insert(neighbor_id.clone(), inner);
            }
        }
        neighbor_ids_map.insert(node_id, neighbor_ids);
    }

    debug_assert!(head_node_map
        .keys()
        .all(|id| !inner_node_map.contains_key(id)));
    debug_assert!(inner_node_map.values().all(|inner| inner.node.is_some()));
    debug_assert!(inner_node_map.values().all(|inner| inner.indegree > 0));

    // Using Kahn's algorithm
    let mut queue: BinaryHeap<T> = head_node_map.into_values().collect();
    let mut result = Vec::new();
    while let Some(node) = queue.pop() {
        let node_id = id_fn(&node);
        result.push(node);
        for neighbor_id in neighbor_ids_map.remove(&node_id).unwrap() {
            let inner = inner_node_map.get_mut(&neighbor_id).unwrap();
            inner.indegree -= 1;
            if inner.indegree == 0 {
                queue.push(inner.node.take().unwrap());
                inner_node_map.remove(&neighbor_id);
            }
        }
    }

    assert!(inner_node_map.is_empty(), "graph has cycle");
    Ok(result)
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
    let neighbors_fn = move |node: &T| to_ok_iter(neighbors_fn(node));
    heads_ok(to_ok_iter(start), id_fn, neighbors_fn).unwrap()
}

/// Finds `Ok` nodes in the start set that are not reachable from other nodes in
/// the start set.
///
/// If `start` or `neighbors_fn()` yields an `Err`, this function terminates and
/// returns the error.
pub fn heads_ok<T, ID, E, II, NI>(
    start: II,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> Result<HashSet<T>, E>
where
    T: Hash + Eq + Clone,
    ID: Hash + Eq,
    II: IntoIterator<Item = Result<T, E>>,
    NI: IntoIterator<Item = Result<T, E>>,
{
    let start: Vec<T> = start.into_iter().try_collect()?;
    let mut reachable: HashSet<T> = start.iter().cloned().collect();
    dfs_ok(start.into_iter().map(Ok), id_fn, |node| {
        let neighbors: Vec<Result<T, E>> = neighbors_fn(node).into_iter().collect();
        for neighbor in neighbors.iter().filter_map(|x| x.as_ref().ok()) {
            reachable.remove(neighbor);
        }
        neighbors
    })
    .try_for_each(|r| r.map(|_| ()))?;
    Ok(reachable)
}

/// Finds the closest common neighbor among the `set1` and `set2`.
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
    let neighbors_fn = move |node: &T| to_ok_iter(neighbors_fn(node));
    closest_common_node_ok(to_ok_iter(set1), to_ok_iter(set2), id_fn, neighbors_fn).unwrap()
}

/// Finds the closest common `Ok` neighbor among the `set1` and `set2`.
///
/// If the traverse reached to an `Err`, this function terminates and returns
/// the error.
pub fn closest_common_node_ok<T, ID, E, II1, II2, NI>(
    set1: II1,
    set2: II2,
    id_fn: impl Fn(&T) -> ID,
    mut neighbors_fn: impl FnMut(&T) -> NI,
) -> Result<Option<T>, E>
where
    ID: Hash + Eq,
    II1: IntoIterator<Item = Result<T, E>>,
    II2: IntoIterator<Item = Result<T, E>>,
    NI: IntoIterator<Item = Result<T, E>>,
{
    let mut visited1 = HashSet::new();
    let mut visited2 = HashSet::new();

    // TODO: might be better to leave an Err so long as the work contains at
    // least one Ok node. If a work1 node is included in visited2, it should be
    // the closest node even if work2 had previously contained an Err.
    let mut work1: Vec<Result<T, E>> = set1.into_iter().collect();
    let mut work2: Vec<Result<T, E>> = set2.into_iter().collect();
    while !work1.is_empty() || !work2.is_empty() {
        let mut new_work1 = vec![];
        for node in work1 {
            let node = node?;
            let id: ID = id_fn(&node);
            if visited2.contains(&id) {
                return Ok(Some(node));
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
            let node = node?;
            let id: ID = id_fn(&node);
            if visited1.contains(&id) {
                return Ok(Some(node));
            }
            if visited2.insert(id) {
                for neighbor in neighbors_fn(&node) {
                    new_work2.push(neighbor);
                }
            }
        }
        work2 = new_work2;
    }
    Ok(None)
}

fn to_ok_iter<T>(iter: impl IntoIterator<Item = T>) -> impl Iterator<Item = Result<T, Infallible>> {
    iter.into_iter().map(Ok)
}

#[cfg(test)]
mod tests {
    use std::panic;

    use maplit::{hashmap, hashset};

    use super::*;

    #[test]
    fn test_dfs_ok() {
        let neighbors = hashmap! {
            'A' => vec![],
            'B' => vec![Ok('A'), Err('X')],
            'C' => vec![Ok('B')],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        // Self and neighbor nodes shouldn't be lost at the error.
        let nodes = dfs_ok([Ok('C')], id_fn, neighbors_fn).collect_vec();
        assert_eq!(nodes, [Ok('C'), Ok('B'), Err('X'), Ok('A')]);
    }

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

        let common = topo_order_reverse_ord(vec!['C'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['C', 'B', 'A']);
        let common = topo_order_reverse_ord(vec!['C', 'B'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['C', 'B', 'A']);
        let common = topo_order_reverse_ord(vec!['B', 'C'], id_fn, neighbors_fn);
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

        let common = topo_order_reverse_ord(vec!['F'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['F', 'E', 'D', 'C', 'B', 'A']);
        let common = topo_order_reverse_ord(vec!['F', 'E', 'C'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['F', 'E', 'D', 'C', 'B', 'A']);
        let common = topo_order_reverse_ord(vec!['F', 'D', 'E'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['F', 'E', 'D', 'C', 'B', 'A']);
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

        let common = topo_order_reverse_ord(vec!['I'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['I', 'H', 'G', 'F', 'E', 'D', 'C', 'B', 'A']);
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

        let common = topo_order_reverse_ord(vec!['I'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['I', 'h', 'f', 'G', 'e', 'D', 'b', 'C', 'A']);
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

        let common = topo_order_reverse_ord(vec!['E'], id_fn, neighbors_fn);
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

        let common = topo_order_reverse_ord(vec!['G'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['G', 'F', 'E', 'D', 'C', 'B', 'A']);

        // Iterator can be lazy for linear chunks.
        let neighbors_fn = |node: &char| to_ok_iter(neighbors[node].iter().copied());
        let mut inner_iter = TopoOrderReverseLazyInner::empty();
        inner_iter.extend([Ok('G')]);
        assert_eq!(inner_iter.next(id_fn, neighbors_fn), Some(Ok('G')));
        assert!(!inner_iter.start.is_empty());
        assert!(inner_iter.result.is_empty());
        assert_eq!(
            iter::from_fn(|| inner_iter.next(id_fn, neighbors_fn))
                .take(4)
                .collect_vec(),
            ['E', 'F', 'D', 'C'].map(Ok),
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

        let common = topo_order_reverse_ord(vec!['G'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['G', 'F', 'E', 'D', 'c', 'B', 'A']);

        // Iterator can be lazy for linear chunks. The node 'c' is visited before 'D',
        // but it will be processed lazily.
        let neighbors_fn = |node: &char| to_ok_iter(neighbors[node].iter().copied());
        let mut inner_iter = TopoOrderReverseLazyInner::empty();
        inner_iter.extend([Ok('G')]);
        assert_eq!(inner_iter.next(id_fn, neighbors_fn), Some(Ok('G')));
        assert!(!inner_iter.start.is_empty());
        assert!(inner_iter.result.is_empty());
        assert_eq!(
            iter::from_fn(|| inner_iter.next(id_fn, neighbors_fn))
                .take(4)
                .collect_vec(),
            ['F', 'E', 'D', 'c'].map(Ok),
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

        let common = topo_order_reverse_ord(vec!['G'], id_fn, neighbors_fn);
        assert_eq!(common, vec!['G', 'f', 'e', 'd', 'C', 'B', 'A']);

        // Iterator can be lazy for linear chunks.
        let neighbors_fn = |node: &char| to_ok_iter(neighbors[node].iter().copied());
        let mut inner_iter = TopoOrderReverseLazyInner::empty();
        inner_iter.extend([Ok('G')]);
        assert_eq!(inner_iter.next(id_fn, neighbors_fn), Some(Ok('G')));
        assert!(!inner_iter.start.is_empty());
        assert!(inner_iter.result.is_empty());
        assert_eq!(
            iter::from_fn(|| inner_iter.next(id_fn, neighbors_fn))
                .take(4)
                .collect_vec(),
            ['f', 'e', 'd', 'C'].map(Ok),
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

        let common = topo_order_reverse_ord(vec!['F', 'C'], id_fn, neighbors_fn);
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

        let common = topo_order_reverse_ord(vec!['D'], id_fn, neighbors_fn);
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

        let result = panic::catch_unwind(|| topo_order_reverse_ord(vec!['C'], id_fn, neighbors_fn));
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

        let result = panic::catch_unwind(|| topo_order_reverse_ord(vec!['D'], id_fn, neighbors_fn));
        assert!(result.is_err());
    }

    #[test]
    fn test_topo_order_ok() {
        let neighbors = hashmap! {
            'A' => vec![Err('Y')],
            'B' => vec![Ok('A'), Err('X')],
            'C' => vec![Ok('B')],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        // Terminates at Err('X') no matter if the sorting order is forward or
        // reverse. The visiting order matters.
        let result = topo_order_forward_ok([Ok('C')], id_fn, neighbors_fn);
        assert_eq!(result, Err('X'));
        let result = topo_order_reverse_ok([Ok('C')], id_fn, neighbors_fn);
        assert_eq!(result, Err('X'));
        let nodes = topo_order_reverse_lazy_ok([Ok('C')], id_fn, neighbors_fn).collect_vec();
        assert_eq!(nodes, [Ok('C'), Ok('B'), Err('X')]);
        let result = topo_order_reverse_ord_ok([Ok('C')], id_fn, neighbors_fn);
        assert_eq!(result, Err('X'));
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
    fn test_closest_common_node_ok() {
        let neighbors = hashmap! {
            'A' => vec![Err('Y')],
            'B' => vec![Ok('A')],
            'C' => vec![Ok('A')],
            'D' => vec![Err('X')],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let result = closest_common_node_ok([Ok('B')], [Ok('C')], id_fn, neighbors_fn);
        assert_eq!(result, Ok(Some('A')));
        let result = closest_common_node_ok([Ok('C')], [Ok('D')], id_fn, neighbors_fn);
        assert_eq!(result, Err('X'));
        let result = closest_common_node_ok([Ok('C')], [Err('Z')], id_fn, neighbors_fn);
        assert_eq!(result, Err('Z'));
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

    #[test]
    fn test_heads_ok() {
        let neighbors = hashmap! {
            'A' => vec![],
            'B' => vec![Ok('A'), Err('X')],
            'C' => vec![Ok('B')],
        };
        let id_fn = |node: &char| *node;
        let neighbors_fn = |node: &char| neighbors[node].clone();

        let result = heads_ok([Ok('C')], id_fn, neighbors_fn);
        assert_eq!(result, Err('X'));
        let result = heads_ok([Ok('B')], id_fn, neighbors_fn);
        assert_eq!(result, Err('X'));
        let result = heads_ok([Ok('A')], id_fn, neighbors_fn);
        assert_eq!(result, Ok(hashset! {'A'}));
    }
}
