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
