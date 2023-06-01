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

use std::collections::BTreeMap;
use std::sync::Arc;

use itertools::Itertools as _;

use crate::backend;
use crate::backend::{TreeId, TreeValue};
use crate::repo_path::{RepoPath, RepoPathJoin};
use crate::store::Store;
use crate::tree::Tree;

#[derive(Debug)]
enum Override {
    Tombstone,
    Replace(TreeValue),
}

#[derive(Debug)]
pub struct TreeBuilder {
    store: Arc<Store>,
    base_tree_id: TreeId,
    overrides: BTreeMap<RepoPath, Override>,
}

impl TreeBuilder {
    pub fn new(store: Arc<Store>, base_tree_id: TreeId) -> TreeBuilder {
        let overrides = BTreeMap::new();
        TreeBuilder {
            store,
            base_tree_id,
            overrides,
        }
    }

    pub fn store(&self) -> &Store {
        self.store.as_ref()
    }

    pub fn has_overrides(&self) -> bool {
        !self.overrides.is_empty()
    }

    pub fn set(&mut self, path: RepoPath, value: TreeValue) {
        self.overrides.insert(path, Override::Replace(value));
    }

    pub fn remove(&mut self, path: RepoPath) {
        self.overrides.insert(path, Override::Tombstone);
    }

    pub fn write_tree(self) -> TreeId {
        let mut trees_to_write = self.get_base_trees();
        if trees_to_write.is_empty() {
            return self.base_tree_id;
        }

        // Update entries in parent trees for file overrides
        for (path, file_override) in self.overrides {
            if let Some((dir, basename)) = path.split() {
                let tree = trees_to_write.get_mut(&dir).unwrap();
                match file_override {
                    Override::Replace(value) => {
                        tree.set(basename.clone(), value);
                    }
                    Override::Tombstone => {
                        tree.remove(basename);
                    }
                }
            }
        }

        // Write trees in reverse lexicographical order, starting with trees without
        // children.
        let store = self.store.as_ref();
        // TODO: trees_to_write.pop_last() can be used, but requires Rust 1.66.0
        let mut dirs_to_write = trees_to_write.keys().cloned().collect_vec();
        while let Some(dir) = dirs_to_write.pop() {
            let tree = trees_to_write.remove(&dir).unwrap();
            if let Some((parent, basename)) = dir.split() {
                let parent_tree = trees_to_write.get_mut(&parent).unwrap();
                if tree.is_empty() {
                    if let Some(TreeValue::Tree(_)) = parent_tree.value(basename) {
                        parent_tree.remove(basename);
                    } else {
                        // Entry would have been replaced with file (see above)
                    }
                } else {
                    let tree_id = store.write_tree(&dir, &tree).unwrap();
                    parent_tree.set(basename.clone(), TreeValue::Tree(tree_id));
                }
            } else {
                // We're writing the root tree. Write it even if empty. Return its id.
                assert!(dirs_to_write.is_empty());
                return store.write_tree(&dir, &tree).unwrap();
            }
        }

        unreachable!("trees_to_write must contain the root tree");
    }

    fn get_base_trees(&self) -> BTreeMap<RepoPath, backend::Tree> {
        let mut tree_cache = BTreeMap::new();
        let store = self.store.clone();

        let mut populate_trees = |dir: &RepoPath| {
            let mut current_dir = RepoPath::root();

            if !tree_cache.contains_key(&current_dir) {
                let tree = store.get_tree(&current_dir, &self.base_tree_id).unwrap();
                tree_cache.insert(current_dir.clone(), tree);
            }

            for component in dir.components() {
                let next_dir = current_dir.join(component);
                let current_tree = tree_cache.get(&current_dir).unwrap();
                if !tree_cache.contains_key(&next_dir) {
                    let tree = current_tree
                        .sub_tree(component)
                        .unwrap_or_else(|| Tree::null(self.store.clone(), next_dir.clone()));
                    tree_cache.insert(next_dir.clone(), tree);
                }
                current_dir = next_dir;
            }
        };
        for path in self.overrides.keys() {
            if let Some(parent) = path.parent() {
                populate_trees(&parent);
            }
        }

        tree_cache
            .into_iter()
            .map(|(dir, tree)| (dir, tree.data().clone()))
            .collect()
    }
}
