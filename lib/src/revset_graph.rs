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

use crate::backend::CommitId;

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct RevsetGraphEdge {
    pub target: CommitId,
    pub edge_type: RevsetGraphEdgeType,
}

impl RevsetGraphEdge {
    pub fn missing(target: CommitId) -> Self {
        Self {
            target,
            edge_type: RevsetGraphEdgeType::Missing,
        }
    }

    pub fn direct(target: CommitId) -> Self {
        Self {
            target,
            edge_type: RevsetGraphEdgeType::Direct,
        }
    }

    pub fn indirect(target: CommitId) -> Self {
        Self {
            target,
            edge_type: RevsetGraphEdgeType::Indirect,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum RevsetGraphEdgeType {
    Missing,
    Direct,
    Indirect,
}

pub struct ReverseRevsetGraphIterator {
    items: Vec<(CommitId, Vec<RevsetGraphEdge>)>,
}

impl ReverseRevsetGraphIterator {
    pub fn new<'revset>(
        input: Box<dyn Iterator<Item = (CommitId, Vec<RevsetGraphEdge>)> + 'revset>,
    ) -> Self {
        let mut entries = vec![];
        let mut reverse_edges: HashMap<CommitId, Vec<RevsetGraphEdge>> = HashMap::new();
        for (commit_id, edges) in input {
            for RevsetGraphEdge { target, edge_type } in edges {
                reverse_edges
                    .entry(target)
                    .or_default()
                    .push(RevsetGraphEdge {
                        target: commit_id.clone(),
                        edge_type,
                    })
            }
            entries.push(commit_id);
        }

        let mut items = vec![];
        for commit_id in entries.into_iter() {
            let edges = reverse_edges.get(&commit_id).cloned().unwrap_or_default();
            items.push((commit_id, edges));
        }
        Self { items }
    }
}

impl Iterator for ReverseRevsetGraphIterator {
    type Item = (CommitId, Vec<RevsetGraphEdge>);

    fn next(&mut self) -> Option<Self::Item> {
        self.items.pop()
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools as _;
    use renderdag::{Ancestor, GraphRowRenderer, Renderer as _};

    use super::*;
    use crate::backend::ObjectId;

    fn id(c: char) -> CommitId {
        let d = u8::try_from(c).unwrap();
        CommitId::new(vec![d])
    }

    fn missing(c: char) -> RevsetGraphEdge {
        RevsetGraphEdge::missing(id(c))
    }

    fn direct(c: char) -> RevsetGraphEdge {
        RevsetGraphEdge::direct(id(c))
    }

    fn indirect(c: char) -> RevsetGraphEdge {
        RevsetGraphEdge::indirect(id(c))
    }

    fn format_edge(edge: &RevsetGraphEdge) -> String {
        let c = char::from(edge.target.as_bytes()[0]);
        match edge.edge_type {
            RevsetGraphEdgeType::Missing => format!("missing({c})"),
            RevsetGraphEdgeType::Direct => format!("direct({c})"),
            RevsetGraphEdgeType::Indirect => format!("indirect({c})"),
        }
    }

    fn format_graph(
        graph_iter: impl IntoIterator<Item = (CommitId, Vec<RevsetGraphEdge>)>,
    ) -> String {
        let mut renderer = GraphRowRenderer::new()
            .output()
            .with_min_row_height(2)
            .build_box_drawing();
        graph_iter
            .into_iter()
            .map(|(id, edges)| {
                let glyph = char::from(id.as_bytes()[0]).to_string();
                let message = edges.iter().map(format_edge).join(", ");
                let parents = edges
                    .into_iter()
                    .map(|edge| match edge.edge_type {
                        RevsetGraphEdgeType::Missing => Ancestor::Anonymous,
                        RevsetGraphEdgeType::Direct => Ancestor::Parent(edge.target),
                        RevsetGraphEdgeType::Indirect => Ancestor::Ancestor(edge.target),
                    })
                    .collect();
                renderer.next_row(id, parents, glyph, message)
            })
            .collect()
    }

    #[test]
    fn test_format_graph() {
        let graph = vec![
            (id('D'), vec![direct('C'), indirect('B')]),
            (id('C'), vec![direct('A')]),
            (id('B'), vec![missing('X')]),
            (id('A'), vec![]),
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
}
