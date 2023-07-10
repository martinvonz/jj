// Copyright 2021-2022 The Jujutsu Authors
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

use std::fmt::{Debug, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

use crate::lock::FileLock;
use crate::op_heads_store::{OpHeadsStore, OpHeadsStoreLock};
use crate::op_store::OperationId;
use crate::operation::Operation;

pub struct SimpleOpHeadsStore {
    dir: PathBuf,
}

impl Debug for SimpleOpHeadsStore {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimpleOpHeadsStore")
            .field("dir", &self.dir)
            .finish()
    }
}

impl SimpleOpHeadsStore {
    pub fn init(dir: &Path) -> Self {
        let op_heads_dir = dir.join("heads");
        fs::create_dir(&op_heads_dir).unwrap();
        Self { dir: op_heads_dir }
    }

    pub fn load(dir: &Path) -> Self {
        let op_heads_dir = dir.join("heads");
        // TODO: Delete this migration code at 0.9+ or so
        if !op_heads_dir.exists() {
            // For some months during 0.7 development, the name was "simple_op_heads"
            if dir.join("simple_op_heads").exists() {
                fs::rename(dir.join("simple_op_heads"), &op_heads_dir).unwrap();
            } else {
                let old_store = Self {
                    dir: dir.to_path_buf(),
                };
                fs::create_dir(&op_heads_dir).unwrap();
                let new_store = Self { dir: op_heads_dir };

                for id in old_store.get_op_heads() {
                    old_store.remove_op_head(&id);
                    new_store.add_op_head(&id);
                }
                return new_store;
            }
        }

        Self { dir: op_heads_dir }
    }
}

struct SimpleOpHeadsStoreLock<'a> {
    store: &'a dyn OpHeadsStore,
    _lock: FileLock,
}

impl OpHeadsStoreLock<'_> for SimpleOpHeadsStoreLock<'_> {
    fn promote_new_op(&self, new_op: &Operation) {
        self.store.add_op_head(new_op.id());
        for old_id in new_op.parent_ids() {
            self.store.remove_op_head(old_id);
        }
    }
}

impl OpHeadsStore for SimpleOpHeadsStore {
    fn name(&self) -> &str {
        "simple_op_heads_store"
    }

    fn add_op_head(&self, id: &OperationId) {
        std::fs::write(self.dir.join(id.hex()), "").unwrap();
    }

    fn remove_op_head(&self, id: &OperationId) {
        // It's fine if the old head was not found. It probably means
        // that we're on a distributed file system where the locking
        // doesn't work. We'll probably end up with two current
        // heads. We'll detect that next time we load the view.
        std::fs::remove_file(self.dir.join(id.hex())).ok();
    }

    fn get_op_heads(&self) -> Vec<OperationId> {
        let mut op_heads = vec![];
        for op_head_entry in std::fs::read_dir(&self.dir).unwrap() {
            let op_head_file_name = op_head_entry.unwrap().file_name();
            let op_head_file_name = op_head_file_name.to_str().unwrap();
            if let Ok(op_head) = hex::decode(op_head_file_name) {
                op_heads.push(OperationId::new(op_head));
            }
        }
        op_heads
    }

    fn lock<'a>(&'a self) -> Box<dyn OpHeadsStoreLock<'a> + 'a> {
        Box::new(SimpleOpHeadsStoreLock {
            store: self,
            _lock: FileLock::lock(self.dir.join("lock")),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::path::Path;

    use itertools::Itertools;

    use crate::op_heads_store::OpHeadsStore;
    use crate::op_store::OperationId;
    use crate::simple_op_heads_store::SimpleOpHeadsStore;

    fn read_dir(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_str().unwrap().to_string())
            .sorted()
            .collect()
    }

    #[test]
    fn test_simple_op_heads_store_migration_into_subdir() {
        let test_dir = testutils::new_temp_dir();
        let store_path = test_dir.path().join("op_heads");
        fs::create_dir(&store_path).unwrap();

        let op1 = OperationId::from_hex("012345");
        let op2 = OperationId::from_hex("abcdef");
        let mut ops = HashSet::new();
        ops.insert(op1.clone());
        ops.insert(op2.clone());

        let old_store = SimpleOpHeadsStore {
            dir: store_path.clone(),
        };
        old_store.add_op_head(&op1);
        old_store.add_op_head(&op2);

        assert_eq!(vec!["012345", "abcdef"], read_dir(&store_path));
        drop(old_store);

        let new_store = SimpleOpHeadsStore::load(&store_path);
        assert_eq!(&ops, &new_store.get_op_heads().into_iter().collect());
        assert_eq!(vec!["heads"], read_dir(&store_path));
        assert_eq!(
            vec!["012345", "abcdef"],
            read_dir(&store_path.join("heads"))
        );

        // Migration is idempotent
        let new_store = SimpleOpHeadsStore::load(&store_path);
        assert_eq!(&ops, &new_store.get_op_heads().into_iter().collect());
        assert_eq!(vec!["heads"], read_dir(&store_path));
        assert_eq!(
            vec!["012345", "abcdef"],
            read_dir(&store_path.join("heads"))
        );
    }

    #[test]
    fn test_simple_op_heads_store_migration_change_dirname() {
        let test_dir = testutils::new_temp_dir();
        let store_path = test_dir.path().join("op_heads");
        fs::create_dir(&store_path).unwrap();
        let old_heads_path = store_path.join("simple_op_heads");
        fs::create_dir(&old_heads_path).unwrap();

        let op1 = OperationId::from_hex("012345");
        let op2 = OperationId::from_hex("abcdef");
        let mut ops = HashSet::new();
        ops.insert(op1.clone());
        ops.insert(op2.clone());

        let old_store = SimpleOpHeadsStore {
            dir: old_heads_path,
        };
        old_store.add_op_head(&op1);
        old_store.add_op_head(&op2);

        assert_eq!(vec!["simple_op_heads"], read_dir(&store_path));
        drop(old_store);

        let new_store = SimpleOpHeadsStore::load(&store_path);
        assert_eq!(&ops, &new_store.get_op_heads().into_iter().collect());
        assert_eq!(vec!["heads"], read_dir(&store_path));
        assert_eq!(
            vec!["012345", "abcdef"],
            read_dir(&store_path.join("heads"))
        );

        // Migration is idempotent
        let new_store = SimpleOpHeadsStore::load(&store_path);
        assert_eq!(&ops, &new_store.get_op_heads().into_iter().collect());
        assert_eq!(vec!["heads"], read_dir(&store_path));
        assert_eq!(
            vec!["012345", "abcdef"],
            read_dir(&store_path.join("heads"))
        );
    }
}
