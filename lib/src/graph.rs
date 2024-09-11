// Copyright 2021-2023 The Jujutsu Authors
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

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::hash::Hash;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub struct GraphEdge<N> {
    pub target: N,
    pub edge_type: GraphEdgeType,
}

impl<N> GraphEdge<N> {
    pub fn missing(target: N) -> Self {
        Self {
            target,
            edge_type: GraphEdgeType::Missing,
        }
    }

    pub fn direct(target: N) -> Self {
        Self {
            target,
            edge_type: GraphEdgeType::Direct,
        }
    }

    pub fn indirect(target: N) -> Self {
        Self {
            target,
            edge_type: GraphEdgeType::Indirect,
        }
    }

    pub fn map<M>(self, f: impl FnOnce(N) -> M) -> GraphEdge<M> {
        GraphEdge {
            target: f(self.target),
            edge_type: self.edge_type,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum GraphEdgeType {
    Missing,
    Direct,
    Indirect,
}

fn reachable_targets<N>(edges: &[GraphEdge<N>]) -> impl DoubleEndedIterator<Item = &N> {
    edges
        .iter()
        .filter(|edge| edge.edge_type != GraphEdgeType::Missing)
        .map(|edge| &edge.target)
}

pub struct ReverseGraphIterator<N> {
    items: Vec<(N, Vec<GraphEdge<N>>)>,
}

impl<N> ReverseGraphIterator<N>
where
    N: Hash + Eq + Clone,
{
    pub fn new(input: impl IntoIterator<Item = (N, Vec<GraphEdge<N>>)>) -> Self {
        let mut entries = vec![];
        let mut reverse_edges: HashMap<N, Vec<GraphEdge<N>>> = HashMap::new();
        for (node, edges) in input {
            for GraphEdge { target, edge_type } in edges {
                reverse_edges.entry(target).or_default().push(GraphEdge {
                    target: node.clone(),
                    edge_type,
                })
            }
            entries.push(node);
        }

        let mut items = vec![];
        for node in entries.into_iter() {
            let edges = reverse_edges.get(&node).cloned().unwrap_or_default();
            items.push((node, edges));
        }
        Self { items }
    }
}

impl<N> Iterator for ReverseGraphIterator<N> {
    type Item = (N, Vec<GraphEdge<N>>);

    fn next(&mut self) -> Option<Self::Item> {
        self.items.pop()
    }
}

/// Graph iterator adapter to group topological branches.
///
/// Basic idea is DFS from the heads. At fork point, the other descendant
/// branches will be visited. At merge point, the second (or the last) ancestor
/// branch will be visited first. This is practically [the same as Git][Git].
///
/// The branch containing the first commit in the input iterator will be emitted
/// first. It is often the working-copy ancestor branch. The other head branches
/// won't be enqueued eagerly, and will be emitted as late as possible.
///
/// [Git]: https://github.blog/2022-08-30-gits-database-internals-ii-commit-history-queries/#topological-sorting
#[derive(Clone, Debug)]
pub struct TopoGroupedGraphIterator<N, I> {
    input_iter: I,
    /// Graph nodes read from the input iterator but not yet emitted.
    nodes: HashMap<N, TopoGroupedGraphNode<N>>,
    /// Stack of graph nodes to be emitted.
    emittable_ids: Vec<N>,
    /// List of new head nodes found while processing unpopulated nodes.
    new_head_ids: VecDeque<N>,
    /// Set of nodes which may be ancestors of `new_head_ids`.
    blocked_ids: HashSet<N>,
}

#[derive(Clone, Debug)]
struct TopoGroupedGraphNode<N> {
    /// Graph nodes which must be emitted before.
    child_ids: HashSet<N>,
    /// Graph edges to parent nodes. `None` until this node is populated.
    edges: Option<Vec<GraphEdge<N>>>,
}

impl<N> Default for TopoGroupedGraphNode<N> {
    fn default() -> Self {
        Self {
            child_ids: Default::default(),
            edges: None,
        }
    }
}

impl<N, I> TopoGroupedGraphIterator<N, I>
where
    N: Clone + Hash + Eq,
    I: Iterator<Item = (N, Vec<GraphEdge<N>>)>,
{
    /// Wraps the given iterator to group topological branches. The input
    /// iterator must be topologically ordered.
    pub fn new(input_iter: I) -> Self {
        TopoGroupedGraphIterator {
            input_iter,
            nodes: HashMap::new(),
            emittable_ids: Vec::new(),
            new_head_ids: VecDeque::new(),
            blocked_ids: HashSet::new(),
        }
    }

    #[must_use]
    fn populate_one(&mut self) -> Option<()> {
        let (current_id, edges) = self.input_iter.next()?;

        // Set up reverse reference
        for parent_id in reachable_targets(&edges) {
            let parent_node = self.nodes.entry(parent_id.clone()).or_default();
            parent_node.child_ids.insert(current_id.clone());
        }

        // Populate the current node
        if let Some(current_node) = self.nodes.get_mut(&current_id) {
            assert!(current_node.edges.is_none());
            current_node.edges = Some(edges);
        } else {
            let current_node = TopoGroupedGraphNode {
                edges: Some(edges),
                ..Default::default()
            };
            self.nodes.insert(current_id.clone(), current_node);
            // Push to non-emitting list so the new head wouldn't be interleaved
            self.new_head_ids.push_back(current_id);
        }

        Some(())
    }

    /// Enqueues the first new head which will unblock the waiting ancestors.
    ///
    /// This does not move multiple head nodes to the queue at once because
    /// heads may be connected to the fork points in arbitrary order.
    fn flush_new_head(&mut self) {
        assert!(!self.new_head_ids.is_empty());
        if self.blocked_ids.is_empty() || self.new_head_ids.len() <= 1 {
            // Fast path: orphaned or no choice
            let new_head_id = self.new_head_ids.pop_front().unwrap();
            self.emittable_ids.push(new_head_id);
            self.blocked_ids.clear();
            return;
        }

        // Mark descendant nodes reachable from the blocking nodes
        let mut to_visit: Vec<&N> = self
            .blocked_ids
            .iter()
            .map(|id| {
                // Borrow from self.nodes so self.blocked_ids can be mutated later
                let (id, _) = self.nodes.get_key_value(id).unwrap();
                id
            })
            .collect();
        let mut visited: HashSet<&N> = to_visit.iter().copied().collect();
        while let Some(id) = to_visit.pop() {
            let node = self.nodes.get(id).unwrap();
            to_visit.extend(node.child_ids.iter().filter(|id| visited.insert(id)));
        }

        // Pick the first reachable head
        let index = self
            .new_head_ids
            .iter()
            .position(|id| visited.contains(id))
            .expect("blocking head should exist");
        let new_head_id = self.new_head_ids.remove(index).unwrap();

        // Unmark ancestors of the selected head so they won't contribute to future
        // new-head resolution within the newly-unblocked sub graph. The sub graph
        // can have many fork points, and the corresponding heads should be picked in
        // the fork-point order, not in the head appearance order.
        to_visit.push(&new_head_id);
        visited.remove(&new_head_id);
        while let Some(id) = to_visit.pop() {
            let node = self.nodes.get(id).unwrap();
            if let Some(edges) = &node.edges {
                to_visit.extend(reachable_targets(edges).filter(|id| visited.remove(id)));
            }
        }
        self.blocked_ids.retain(|id| visited.contains(id));
        self.emittable_ids.push(new_head_id);
    }

    #[must_use]
    fn next_node(&mut self) -> Option<(N, Vec<GraphEdge<N>>)> {
        // Based on Kahn's algorithm
        loop {
            if let Some(current_id) = self.emittable_ids.last() {
                let Some(current_node) = self.nodes.get_mut(current_id) else {
                    // Queued twice because new children populated and emitted
                    self.emittable_ids.pop().unwrap();
                    continue;
                };
                if !current_node.child_ids.is_empty() {
                    // New children populated after emitting the other
                    let current_id = self.emittable_ids.pop().unwrap();
                    self.blocked_ids.insert(current_id);
                    continue;
                }
                let Some(edges) = current_node.edges.take() else {
                    // Not yet populated
                    self.populate_one().expect("parent node should exist");
                    continue;
                };
                // The second (or the last) parent will be visited first
                let current_id = self.emittable_ids.pop().unwrap();
                self.nodes.remove(&current_id).unwrap();
                for parent_id in reachable_targets(&edges) {
                    let parent_node = self.nodes.get_mut(parent_id).unwrap();
                    parent_node.child_ids.remove(&current_id);
                    if parent_node.child_ids.is_empty() {
                        let reusable_id = self.blocked_ids.take(parent_id);
                        let parent_id = reusable_id.unwrap_or_else(|| parent_id.clone());
                        self.emittable_ids.push(parent_id);
                    } else {
                        self.blocked_ids.insert(parent_id.clone());
                    }
                }
                return Some((current_id, edges));
            } else if !self.new_head_ids.is_empty() {
                self.flush_new_head();
            } else {
                // Populate the first or orphan head
                self.populate_one()?;
            }
        }
    }
}

impl<N, I> Iterator for TopoGroupedGraphIterator<N, I>
where
    N: Clone + Hash + Eq,
    I: Iterator<Item = (N, Vec<GraphEdge<N>>)>,
{
    type Item = (N, Vec<GraphEdge<N>>);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(node) = self.next_node() {
            Some(node)
        } else {
            assert!(self.nodes.is_empty(), "all nodes should have been emitted");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools as _;
    use renderdag::Ancestor;
    use renderdag::GraphRowRenderer;
    use renderdag::Renderer as _;

    use super::*;

    fn missing(c: char) -> GraphEdge<char> {
        GraphEdge::missing(c)
    }

    fn direct(c: char) -> GraphEdge<char> {
        GraphEdge::direct(c)
    }

    fn indirect(c: char) -> GraphEdge<char> {
        GraphEdge::indirect(c)
    }

    fn format_edge(edge: &GraphEdge<char>) -> String {
        let c = edge.target;
        match edge.edge_type {
            GraphEdgeType::Missing => format!("missing({c})"),
            GraphEdgeType::Direct => format!("direct({c})"),
            GraphEdgeType::Indirect => format!("indirect({c})"),
        }
    }

    fn format_graph(graph_iter: impl IntoIterator<Item = (char, Vec<GraphEdge<char>>)>) -> String {
        let mut renderer = GraphRowRenderer::new()
            .output()
            .with_min_row_height(2)
            .build_box_drawing();
        graph_iter
            .into_iter()
            .map(|(id, edges)| {
                let glyph = id.to_string();
                let message = edges.iter().map(format_edge).join(", ");
                let parents = edges
                    .into_iter()
                    .map(|edge| match edge.edge_type {
                        GraphEdgeType::Missing => Ancestor::Anonymous,
                        GraphEdgeType::Direct => Ancestor::Parent(edge.target),
                        GraphEdgeType::Indirect => Ancestor::Ancestor(edge.target),
                    })
                    .collect();
                renderer.next_row(id, parents, glyph, message)
            })
            .collect()
    }

    #[test]
    fn test_format_graph() {
        let graph = vec![
            ('D', vec![direct('C'), indirect('B')]),
            ('C', vec![direct('A')]),
            ('B', vec![missing('X')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph), @r###"
        D    direct(C), indirect(B)
        ├─╮
        C ╷  direct(A)
        │ ╷
        │ B  missing(X)
        │ │
        │ ~
        │
        A

        "###);
    }

    fn topo_grouped<I>(graph_iter: I) -> TopoGroupedGraphIterator<char, I::IntoIter>
    where
        I: IntoIterator<Item = (char, Vec<GraphEdge<char>>)>,
    {
        TopoGroupedGraphIterator::new(graph_iter.into_iter())
    }

    #[test]
    fn test_topo_grouped_multiple_roots() {
        let graph = [
            ('C', vec![missing('Y')]),
            ('B', vec![missing('X')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        C  missing(Y)
        │
        ~

        B  missing(X)
        │
        ~

        A
        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        C  missing(Y)
        │
        ~

        B  missing(X)
        │
        ~

        A
        "###);

        // All nodes can be lazily emitted.
        let mut iter = topo_grouped(graph.iter().cloned().peekable());
        assert_eq!(iter.next().unwrap().0, 'C');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'B');
        assert_eq!(iter.next().unwrap().0, 'B');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'A');
    }

    #[test]
    fn test_topo_grouped_trivial_fork() {
        let graph = [
            ('E', vec![direct('B')]),
            ('D', vec![direct('A')]),
            ('C', vec![direct('B')]),
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        E  direct(B)
        │
        │ D  direct(A)
        │ │
        │ │ C  direct(B)
        ├───╯
        B │  direct(A)
        ├─╯
        A

        "###);
        // D-A is found earlier than B-A, but B is emitted first because it belongs to
        // the emitting branch.
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        E  direct(B)
        │
        │ C  direct(B)
        ├─╯
        B  direct(A)
        │
        │ D  direct(A)
        ├─╯
        A

        "###);

        // E can be lazy, then D and C will be queued.
        let mut iter = topo_grouped(graph.iter().cloned().peekable());
        assert_eq!(iter.next().unwrap().0, 'E');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'D');
        assert_eq!(iter.next().unwrap().0, 'C');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'B');
        assert_eq!(iter.next().unwrap().0, 'B');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'A');
    }

    #[test]
    fn test_topo_grouped_fork_interleaved() {
        let graph = [
            ('F', vec![direct('D')]),
            ('E', vec![direct('C')]),
            ('D', vec![direct('B')]),
            ('C', vec![direct('B')]),
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        F  direct(D)
        │
        │ E  direct(C)
        │ │
        D │  direct(B)
        │ │
        │ C  direct(B)
        ├─╯
        B  direct(A)
        │
        A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        F  direct(D)
        │
        D  direct(B)
        │
        │ E  direct(C)
        │ │
        │ C  direct(B)
        ├─╯
        B  direct(A)
        │
        A

        "###);

        // F can be lazy, then E will be queued, then C.
        let mut iter = topo_grouped(graph.iter().cloned().peekable());
        assert_eq!(iter.next().unwrap().0, 'F');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'E');
        assert_eq!(iter.next().unwrap().0, 'D');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'C');
        assert_eq!(iter.next().unwrap().0, 'E');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'B');
    }

    #[test]
    fn test_topo_grouped_fork_multiple_heads() {
        let graph = vec![
            ('I', vec![direct('E')]),
            ('H', vec![direct('C')]),
            ('G', vec![direct('A')]),
            ('F', vec![direct('E')]),
            ('E', vec![direct('C')]),
            ('D', vec![direct('C')]),
            ('C', vec![direct('A')]),
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        I  direct(E)
        │
        │ H  direct(C)
        │ │
        │ │ G  direct(A)
        │ │ │
        │ │ │ F  direct(E)
        ├─────╯
        E │ │  direct(C)
        ├─╯ │
        │ D │  direct(C)
        ├─╯ │
        C   │  direct(A)
        ├───╯
        │ B  direct(A)
        ├─╯
        A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        I  direct(E)
        │
        │ F  direct(E)
        ├─╯
        E  direct(C)
        │
        │ H  direct(C)
        ├─╯
        │ D  direct(C)
        ├─╯
        C  direct(A)
        │
        │ G  direct(A)
        ├─╯
        │ B  direct(A)
        ├─╯
        A

        "###);

        // I can be lazy, then H, G, and F will be queued.
        let mut iter = topo_grouped(graph.iter().cloned().peekable());
        assert_eq!(iter.next().unwrap().0, 'I');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'H');
        assert_eq!(iter.next().unwrap().0, 'F');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'E');
    }

    #[test]
    fn test_topo_grouped_fork_parallel() {
        let graph = vec![
            // Pull all sub graphs in reverse order:
            ('I', vec![direct('A')]),
            ('H', vec![direct('C')]),
            ('G', vec![direct('E')]),
            // Orphan sub graph G,F-E:
            ('F', vec![direct('E')]),
            ('E', vec![missing('Y')]),
            // Orphan sub graph H,D-C:
            ('D', vec![direct('C')]),
            ('C', vec![missing('X')]),
            // Orphan sub graph I,B-A:
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        I  direct(A)
        │
        │ H  direct(C)
        │ │
        │ │ G  direct(E)
        │ │ │
        │ │ │ F  direct(E)
        │ │ ├─╯
        │ │ E  missing(Y)
        │ │ │
        │ │ ~
        │ │
        │ │ D  direct(C)
        │ ├─╯
        │ C  missing(X)
        │ │
        │ ~
        │
        │ B  direct(A)
        ├─╯
        A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        I  direct(A)
        │
        │ B  direct(A)
        ├─╯
        A

        H  direct(C)
        │
        │ D  direct(C)
        ├─╯
        C  missing(X)
        │
        ~

        G  direct(E)
        │
        │ F  direct(E)
        ├─╯
        E  missing(Y)
        │
        ~
        "###);
    }

    #[test]
    fn test_topo_grouped_fork_nested() {
        fn sub_graph(
            chars: impl IntoIterator<Item = char>,
            root_edges: Vec<GraphEdge<char>>,
        ) -> Vec<(char, Vec<GraphEdge<char>>)> {
            let [b, c, d, e, f]: [char; 5] = chars.into_iter().collect_vec().try_into().unwrap();
            vec![
                (f, vec![direct(c)]),
                (e, vec![direct(b)]),
                (d, vec![direct(c)]),
                (c, vec![direct(b)]),
                (b, root_edges),
            ]
        }

        // One nested fork sub graph from A
        let graph = itertools::chain!(
            vec![('G', vec![direct('A')])],
            sub_graph('B'..='F', vec![direct('A')]),
            vec![('A', vec![])],
        )
        .collect_vec();
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        G  direct(A)
        │
        │ F  direct(C)
        │ │
        │ │ E  direct(B)
        │ │ │
        │ │ │ D  direct(C)
        │ ├───╯
        │ C │  direct(B)
        │ ├─╯
        │ B  direct(A)
        ├─╯
        A

        "###);
        // A::F is picked at A, and A will be unblocked. Then, C::D at C, ...
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        G  direct(A)
        │
        │ F  direct(C)
        │ │
        │ │ D  direct(C)
        │ ├─╯
        │ C  direct(B)
        │ │
        │ │ E  direct(B)
        │ ├─╯
        │ B  direct(A)
        ├─╯
        A

        "###);

        // Two nested fork sub graphs from A
        let graph = itertools::chain!(
            vec![('L', vec![direct('A')])],
            sub_graph('G'..='K', vec![direct('A')]),
            sub_graph('B'..='F', vec![direct('A')]),
            vec![('A', vec![])],
        )
        .collect_vec();
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        L  direct(A)
        │
        │ K  direct(H)
        │ │
        │ │ J  direct(G)
        │ │ │
        │ │ │ I  direct(H)
        │ ├───╯
        │ H │  direct(G)
        │ ├─╯
        │ G  direct(A)
        ├─╯
        │ F  direct(C)
        │ │
        │ │ E  direct(B)
        │ │ │
        │ │ │ D  direct(C)
        │ ├───╯
        │ C │  direct(B)
        │ ├─╯
        │ B  direct(A)
        ├─╯
        A

        "###);
        // A::K is picked at A, and A will be unblocked. Then, H::I at H, ...
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        L  direct(A)
        │
        │ K  direct(H)
        │ │
        │ │ I  direct(H)
        │ ├─╯
        │ H  direct(G)
        │ │
        │ │ J  direct(G)
        │ ├─╯
        │ G  direct(A)
        ├─╯
        │ F  direct(C)
        │ │
        │ │ D  direct(C)
        │ ├─╯
        │ C  direct(B)
        │ │
        │ │ E  direct(B)
        │ ├─╯
        │ B  direct(A)
        ├─╯
        A

        "###);

        // Two nested fork sub graphs from A, interleaved
        let graph = itertools::chain!(
            vec![('L', vec![direct('A')])],
            sub_graph(['C', 'E', 'G', 'I', 'K'], vec![direct('A')]),
            sub_graph(['B', 'D', 'F', 'H', 'J'], vec![direct('A')]),
            vec![('A', vec![])],
        )
        .sorted_by(|(id1, _), (id2, _)| id2.cmp(id1))
        .collect_vec();
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        L  direct(A)
        │
        │ K  direct(E)
        │ │
        │ │ J  direct(D)
        │ │ │
        │ │ │ I  direct(C)
        │ │ │ │
        │ │ │ │ H  direct(B)
        │ │ │ │ │
        │ │ │ │ │ G  direct(E)
        │ ├───────╯
        │ │ │ │ │ F  direct(D)
        │ │ ├─────╯
        │ E │ │ │  direct(C)
        │ ├───╯ │
        │ │ D   │  direct(B)
        │ │ ├───╯
        │ C │  direct(A)
        ├─╯ │
        │   B  direct(A)
        ├───╯
        A

        "###);
        // A::K is picked at A, and A will be unblocked. Then, E::G at E, ...
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        L  direct(A)
        │
        │ K  direct(E)
        │ │
        │ │ G  direct(E)
        │ ├─╯
        │ E  direct(C)
        │ │
        │ │ I  direct(C)
        │ ├─╯
        │ C  direct(A)
        ├─╯
        │ J  direct(D)
        │ │
        │ │ F  direct(D)
        │ ├─╯
        │ D  direct(B)
        │ │
        │ │ H  direct(B)
        │ ├─╯
        │ B  direct(A)
        ├─╯
        A

        "###);

        // Merged fork sub graphs at K
        let graph = itertools::chain!(
            vec![('K', vec![direct('E'), direct('J')])],
            sub_graph('F'..='J', vec![missing('Y')]),
            sub_graph('A'..='E', vec![missing('X')]),
        )
        .collect_vec();
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        K    direct(E), direct(J)
        ├─╮
        │ J  direct(G)
        │ │
        │ │ I  direct(F)
        │ │ │
        │ │ │ H  direct(G)
        │ ├───╯
        │ G │  direct(F)
        │ ├─╯
        │ F  missing(Y)
        │ │
        │ ~
        │
        E  direct(B)
        │
        │ D  direct(A)
        │ │
        │ │ C  direct(B)
        ├───╯
        B │  direct(A)
        ├─╯
        A  missing(X)
        │
        ~
        "###);
        // K-E,J is resolved without queuing new heads. Then, G::H, F::I, B::C, and
        // A::D.
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        K    direct(E), direct(J)
        ├─╮
        │ J  direct(G)
        │ │
        E │  direct(B)
        │ │
        │ │ H  direct(G)
        │ ├─╯
        │ G  direct(F)
        │ │
        │ │ I  direct(F)
        │ ├─╯
        │ F  missing(Y)
        │ │
        │ ~
        │
        │ C  direct(B)
        ├─╯
        B  direct(A)
        │
        │ D  direct(A)
        ├─╯
        A  missing(X)
        │
        ~
        "###);

        // Merged fork sub graphs at K, interleaved
        let graph = itertools::chain!(
            vec![('K', vec![direct('I'), direct('J')])],
            sub_graph(['B', 'D', 'F', 'H', 'J'], vec![missing('Y')]),
            sub_graph(['A', 'C', 'E', 'G', 'I'], vec![missing('X')]),
        )
        .sorted_by(|(id1, _), (id2, _)| id2.cmp(id1))
        .collect_vec();
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        K    direct(I), direct(J)
        ├─╮
        │ J  direct(D)
        │ │
        I │  direct(C)
        │ │
        │ │ H  direct(B)
        │ │ │
        │ │ │ G  direct(A)
        │ │ │ │
        │ │ │ │ F  direct(D)
        │ ├─────╯
        │ │ │ │ E  direct(C)
        ├───────╯
        │ D │ │  direct(B)
        │ ├─╯ │
        C │   │  direct(A)
        ├─────╯
        │ B  missing(Y)
        │ │
        │ ~
        │
        A  missing(X)
        │
        ~
        "###);
        // K-I,J is resolved without queuing new heads. Then, D::F, B::H, C::E, and
        // A::G.
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        K    direct(I), direct(J)
        ├─╮
        │ J  direct(D)
        │ │
        I │  direct(C)
        │ │
        │ │ F  direct(D)
        │ ├─╯
        │ D  direct(B)
        │ │
        │ │ H  direct(B)
        │ ├─╯
        │ B  missing(Y)
        │ │
        │ ~
        │
        │ E  direct(C)
        ├─╯
        C  direct(A)
        │
        │ G  direct(A)
        ├─╯
        A  missing(X)
        │
        ~
        "###);
    }

    #[test]
    fn test_topo_grouped_merge_interleaved() {
        let graph = [
            ('F', vec![direct('E')]),
            ('E', vec![direct('C'), direct('D')]),
            ('D', vec![direct('B')]),
            ('C', vec![direct('A')]),
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        F  direct(E)
        │
        E    direct(C), direct(D)
        ├─╮
        │ D  direct(B)
        │ │
        C │  direct(A)
        │ │
        │ B  direct(A)
        ├─╯
        A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        F  direct(E)
        │
        E    direct(C), direct(D)
        ├─╮
        │ D  direct(B)
        │ │
        │ B  direct(A)
        │ │
        C │  direct(A)
        ├─╯
        A

        "###);

        // F, E, and D can be lazy, then C will be queued, then B.
        let mut iter = topo_grouped(graph.iter().cloned().peekable());
        assert_eq!(iter.next().unwrap().0, 'F');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'E');
        assert_eq!(iter.next().unwrap().0, 'E');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'D');
        assert_eq!(iter.next().unwrap().0, 'D');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'C');
        assert_eq!(iter.next().unwrap().0, 'B');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'A');
    }

    #[test]
    fn test_topo_grouped_merge_but_missing() {
        let graph = [
            ('E', vec![direct('D')]),
            ('D', vec![missing('Y'), direct('C')]),
            ('C', vec![direct('B'), missing('X')]),
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        E  direct(D)
        │
        D    missing(Y), direct(C)
        ├─╮
        │ │
        ~ │
          │
          C  direct(B), missing(X)
        ╭─┤
        │ │
        ~ │
          │
          B  direct(A)
          │
          A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        E  direct(D)
        │
        D    missing(Y), direct(C)
        ├─╮
        │ │
        ~ │
          │
          C  direct(B), missing(X)
        ╭─┤
        │ │
        ~ │
          │
          B  direct(A)
          │
          A

        "###);

        // All nodes can be lazily emitted.
        let mut iter = topo_grouped(graph.iter().cloned().peekable());
        assert_eq!(iter.next().unwrap().0, 'E');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'D');
        assert_eq!(iter.next().unwrap().0, 'D');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'C');
        assert_eq!(iter.next().unwrap().0, 'C');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'B');
        assert_eq!(iter.next().unwrap().0, 'B');
        assert_eq!(iter.input_iter.peek().unwrap().0, 'A');
    }

    #[test]
    fn test_topo_grouped_merge_criss_cross() {
        let graph = vec![
            ('G', vec![direct('E')]),
            ('F', vec![direct('D')]),
            ('E', vec![direct('B'), direct('C')]),
            ('D', vec![direct('B'), direct('C')]),
            ('C', vec![direct('A')]),
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        G  direct(E)
        │
        │ F  direct(D)
        │ │
        E │    direct(B), direct(C)
        ├───╮
        │ D │  direct(B), direct(C)
        ╭─┴─╮
        │   C  direct(A)
        │   │
        B   │  direct(A)
        ├───╯
        A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        G  direct(E)
        │
        E    direct(B), direct(C)
        ├─╮
        │ │ F  direct(D)
        │ │ │
        │ │ D  direct(B), direct(C)
        ╭─┬─╯
        │ C  direct(A)
        │ │
        B │  direct(A)
        ├─╯
        A

        "###);
    }

    #[test]
    fn test_topo_grouped_merge_descendants_interleaved() {
        let graph = vec![
            ('H', vec![direct('F')]),
            ('G', vec![direct('E')]),
            ('F', vec![direct('D')]),
            ('E', vec![direct('C')]),
            ('D', vec![direct('C'), direct('B')]),
            ('C', vec![direct('A')]),
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        H  direct(F)
        │
        │ G  direct(E)
        │ │
        F │  direct(D)
        │ │
        │ E  direct(C)
        │ │
        D │  direct(C), direct(B)
        ├─╮
        │ C  direct(A)
        │ │
        B │  direct(A)
        ├─╯
        A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        H  direct(F)
        │
        F  direct(D)
        │
        D    direct(C), direct(B)
        ├─╮
        │ B  direct(A)
        │ │
        │ │ G  direct(E)
        │ │ │
        │ │ E  direct(C)
        ├───╯
        C │  direct(A)
        ├─╯
        A

        "###);
    }

    #[test]
    fn test_topo_grouped_merge_multiple_roots() {
        let graph = [
            ('D', vec![direct('C')]),
            ('C', vec![direct('B'), direct('A')]),
            ('B', vec![missing('X')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        D  direct(C)
        │
        C    direct(B), direct(A)
        ├─╮
        B │  missing(X)
        │ │
        ~ │
          │
          A

        "###);
        // A is emitted first because it's the second parent.
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        D  direct(C)
        │
        C    direct(B), direct(A)
        ├─╮
        │ A
        │
        B  missing(X)
        │
        ~
        "###);
    }

    #[test]
    fn test_topo_grouped_merge_stairs() {
        let graph = vec![
            // Merge topic branches one by one:
            ('J', vec![direct('I'), direct('G')]),
            ('I', vec![direct('H'), direct('E')]),
            ('H', vec![direct('D'), direct('F')]),
            // Topic branches:
            ('G', vec![direct('D')]),
            ('F', vec![direct('C')]),
            ('E', vec![direct('B')]),
            // Base nodes:
            ('D', vec![direct('C')]),
            ('C', vec![direct('B')]),
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        J    direct(I), direct(G)
        ├─╮
        I │    direct(H), direct(E)
        ├───╮
        H │ │    direct(D), direct(F)
        ├─────╮
        │ G │ │  direct(D)
        ├─╯ │ │
        │   │ F  direct(C)
        │   │ │
        │   E │  direct(B)
        │   │ │
        D   │ │  direct(C)
        ├─────╯
        C   │  direct(B)
        ├───╯
        B  direct(A)
        │
        A

        "###);
        // Second branches are visited first.
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        J    direct(I), direct(G)
        ├─╮
        │ G  direct(D)
        │ │
        I │    direct(H), direct(E)
        ├───╮
        │ │ E  direct(B)
        │ │ │
        H │ │  direct(D), direct(F)
        ├─╮ │
        F │ │  direct(C)
        │ │ │
        │ D │  direct(C)
        ├─╯ │
        C   │  direct(B)
        ├───╯
        B  direct(A)
        │
        A

        "###);
    }

    #[test]
    fn test_topo_grouped_merge_and_fork() {
        let graph = vec![
            ('J', vec![direct('F')]),
            ('I', vec![direct('E')]),
            ('H', vec![direct('G')]),
            ('G', vec![direct('D'), direct('E')]),
            ('F', vec![direct('C')]),
            ('E', vec![direct('B')]),
            ('D', vec![direct('B')]),
            ('C', vec![direct('A')]),
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        J  direct(F)
        │
        │ I  direct(E)
        │ │
        │ │ H  direct(G)
        │ │ │
        │ │ G  direct(D), direct(E)
        │ ╭─┤
        F │ │  direct(C)
        │ │ │
        │ E │  direct(B)
        │ │ │
        │ │ D  direct(B)
        │ ├─╯
        C │  direct(A)
        │ │
        │ B  direct(A)
        ├─╯
        A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        J  direct(F)
        │
        F  direct(C)
        │
        C  direct(A)
        │
        │ I  direct(E)
        │ │
        │ │ H  direct(G)
        │ │ │
        │ │ G  direct(D), direct(E)
        │ ╭─┤
        │ E │  direct(B)
        │ │ │
        │ │ D  direct(B)
        │ ├─╯
        │ B  direct(A)
        ├─╯
        A

        "###);
    }

    #[test]
    fn test_topo_grouped_merge_and_fork_multiple_roots() {
        let graph = vec![
            ('J', vec![direct('F')]),
            ('I', vec![direct('G')]),
            ('H', vec![direct('E')]),
            ('G', vec![direct('E'), direct('B')]),
            ('F', vec![direct('D')]),
            ('E', vec![direct('C')]),
            ('D', vec![direct('A')]),
            ('C', vec![direct('A')]),
            ('B', vec![missing('X')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        J  direct(F)
        │
        │ I  direct(G)
        │ │
        │ │ H  direct(E)
        │ │ │
        │ G │  direct(E), direct(B)
        │ ├─╮
        F │ │  direct(D)
        │ │ │
        │ │ E  direct(C)
        │ │ │
        D │ │  direct(A)
        │ │ │
        │ │ C  direct(A)
        ├───╯
        │ B  missing(X)
        │ │
        │ ~
        │
        A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        J  direct(F)
        │
        F  direct(D)
        │
        D  direct(A)
        │
        │ I  direct(G)
        │ │
        │ G    direct(E), direct(B)
        │ ├─╮
        │ │ B  missing(X)
        │ │ │
        │ │ ~
        │ │
        │ │ H  direct(E)
        │ ├─╯
        │ E  direct(C)
        │ │
        │ C  direct(A)
        ├─╯
        A

        "###);
    }

    #[test]
    fn test_topo_grouped_parallel_interleaved() {
        let graph = [
            ('E', vec![direct('C')]),
            ('D', vec![direct('B')]),
            ('C', vec![direct('A')]),
            ('B', vec![missing('X')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        E  direct(C)
        │
        │ D  direct(B)
        │ │
        C │  direct(A)
        │ │
        │ B  missing(X)
        │ │
        │ ~
        │
        A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        E  direct(C)
        │
        C  direct(A)
        │
        A

        D  direct(B)
        │
        B  missing(X)
        │
        ~
        "###);
    }

    #[test]
    fn test_topo_grouped_multiple_child_dependencies() {
        let graph = vec![
            ('I', vec![direct('H'), direct('G')]),
            ('H', vec![direct('D')]),
            ('G', vec![direct('B')]),
            ('F', vec![direct('E'), direct('C')]),
            ('E', vec![direct('D')]),
            ('D', vec![direct('B')]),
            ('C', vec![direct('B')]),
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        I    direct(H), direct(G)
        ├─╮
        H │  direct(D)
        │ │
        │ G  direct(B)
        │ │
        │ │ F    direct(E), direct(C)
        │ │ ├─╮
        │ │ E │  direct(D)
        ├───╯ │
        D │   │  direct(B)
        ├─╯   │
        │     C  direct(B)
        ├─────╯
        B  direct(A)
        │
        A

        "###);
        // Topological order must be preserved. Depending on the implementation,
        // E might be requested more than once by paths D->E and B->D->E.
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        I    direct(H), direct(G)
        ├─╮
        │ G  direct(B)
        │ │
        H │  direct(D)
        │ │
        │ │ F    direct(E), direct(C)
        │ │ ├─╮
        │ │ │ C  direct(B)
        │ ├───╯
        │ │ E  direct(D)
        ├───╯
        D │  direct(B)
        ├─╯
        B  direct(A)
        │
        A

        "###);
    }

    #[test]
    fn test_topo_grouped_requeue_unpopulated() {
        let graph = [
            ('C', vec![direct('A'), direct('B')]),
            ('B', vec![direct('A')]),
            ('A', vec![]),
        ];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        C    direct(A), direct(B)
        ├─╮
        │ B  direct(A)
        ├─╯
        A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        C    direct(A), direct(B)
        ├─╮
        │ B  direct(A)
        ├─╯
        A

        "###);

        // A is queued once by C-A because B isn't populated at this point. Since
        // B is the second parent, B-A is processed next and A is queued again. So
        // one of them in the queue has to be ignored.
        let mut iter = topo_grouped(graph.iter().cloned());
        assert_eq!(iter.next().unwrap().0, 'C');
        assert_eq!(iter.emittable_ids, vec!['A', 'B']);
        assert_eq!(iter.next().unwrap().0, 'B');
        assert_eq!(iter.emittable_ids, vec!['A', 'A']);
        assert_eq!(iter.next().unwrap().0, 'A');
        assert!(iter.next().is_none());
        assert!(iter.emittable_ids.is_empty());
    }

    #[test]
    fn test_topo_grouped_duplicated_edges() {
        // The graph shouldn't have duplicated parent->child edges, but topo-grouped
        // iterator can handle it anyway.
        let graph = [('B', vec![direct('A'), direct('A')]), ('A', vec![])];
        insta::assert_snapshot!(format_graph(graph.iter().cloned()), @r###"
        B  direct(A), direct(A)
        │
        A

        "###);
        insta::assert_snapshot!(format_graph(topo_grouped(graph.iter().cloned())), @r###"
        B  direct(A), direct(A)
        │
        A

        "###);

        let mut iter = topo_grouped(graph.iter().cloned());
        assert_eq!(iter.next().unwrap().0, 'B');
        assert_eq!(iter.emittable_ids, vec!['A', 'A']);
        assert_eq!(iter.next().unwrap().0, 'A');
        assert!(iter.next().is_none());
        assert!(iter.emittable_ids.is_empty());
    }
}
