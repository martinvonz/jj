// Copyright 2024 The Jujutsu Authors
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

//! This module implements a UnionFind<T> type which can be used to
//! efficiently calculate disjoint sets for any data type.

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Clone, Copy)]
struct Node<T> {
    root: T,
    size: u32,
}

/// Implementation of the union-find algorithm:
/// https://en.wikipedia.org/wiki/Disjoint-set_data_structure
///
/// Joins disjoint sets by size to amortize cost.
#[derive(Clone)]
pub struct UnionFind<T> {
    roots: HashMap<T, Node<T>>,
}

impl<T> Default for UnionFind<T>
where
    T: Copy + Eq + Hash,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> UnionFind<T>
where
    T: Copy + Eq + Hash,
{
    /// Creates a new empty UnionFind data structure.
    pub fn new() -> Self {
        Self {
            roots: HashMap::new(),
        }
    }

    /// Returns the root identifying the union this item is a part of.
    pub fn find(&mut self, item: T) -> T {
        self.find_node(item).root
    }

    fn find_node(&mut self, item: T) -> Node<T> {
        match self.roots.get(&item) {
            Some(node) => {
                if node.root != item {
                    let new_root = self.find_node(node.root);
                    self.roots.insert(item, new_root);
                    new_root
                } else {
                    *node
                }
            }
            None => {
                let node = Node::<T> {
                    root: item,
                    size: 1,
                };
                self.roots.insert(item, node);
                node
            }
        }
    }

    /// Unions the disjoint sets connected to `a` and `b`.
    pub fn union(&mut self, a: T, b: T) {
        let a = self.find_node(a);
        let b = self.find_node(b);
        if a.root == b.root {
            return;
        }

        let new_node = Node::<T> {
            root: if a.size < b.size { b.root } else { a.root },
            size: a.size + b.size,
        };
        self.roots.insert(a.root, new_node);
        self.roots.insert(b.root, new_node);
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;

    use super::*;

    #[test]
    fn test_basic() {
        let mut union_find = UnionFind::<i32>::new();

        // Everything starts as a singleton.
        assert_eq!(union_find.find(1), 1);
        assert_eq!(union_find.find(2), 2);
        assert_eq!(union_find.find(3), 3);

        // Make two pair sets. This implicitly adds node 4.
        union_find.union(1, 2);
        union_find.union(3, 4);
        assert_eq!(union_find.find(1), union_find.find(2));
        assert_eq!(union_find.find(3), union_find.find(4));
        assert_ne!(union_find.find(1), union_find.find(3));

        // Unioning the pairs gives everything the same root.
        union_find.union(1, 3);
        assert!([
            union_find.find(1),
            union_find.find(2),
            union_find.find(3),
            union_find.find(4),
        ]
        .iter()
        .all_equal());
    }

    #[test]
    fn test_union_by_size() {
        let mut union_find = UnionFind::<i32>::new();

        // Create a set of 3 and a set of 2.
        union_find.union(1, 2);
        union_find.union(2, 3);
        union_find.union(4, 5);
        let set3 = union_find.find(1);
        let set2 = union_find.find(4);
        assert_ne!(set3, set2);

        // Merging them always chooses the larger set.
        let mut large_first = union_find.clone();
        large_first.union(1, 4);
        assert_eq!(large_first.find(1), set3);
        assert_eq!(large_first.find(4), set3);

        let mut small_first = union_find.clone();
        small_first.union(4, 1);
        assert_eq!(small_first.find(1), set3);
        assert_eq!(small_first.find(4), set3);
    }
}
