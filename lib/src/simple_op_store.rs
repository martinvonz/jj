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

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::fs;
use std::path::PathBuf;

use itertools::Itertools;
use tempfile::PersistError;

use crate::legacy_thrift_op_store::ThriftOpStore;
use crate::op_store::{OpStore, OpStoreError, OpStoreResult, Operation, OperationId, View, ViewId};
use crate::proto_op_store::ProtoOpStore;

impl From<std::io::Error> for OpStoreError {
    fn from(err: std::io::Error) -> Self {
        OpStoreError::Other(err.to_string())
    }
}

impl From<PersistError> for OpStoreError {
    fn from(err: PersistError) -> Self {
        OpStoreError::Other(err.to_string())
    }
}

// TODO: In version 0.7.0 or so, inline ThriftOpStore into this type and drop
// support for upgrading from the proto format
#[derive(Debug)]
pub struct SimpleOpStore {
    delegate: ThriftOpStore,
}

fn upgrade_to_thrift(store_path: PathBuf) -> std::io::Result<()> {
    println!("Upgrading operation log to Thrift format...");
    let old_store = ProtoOpStore::load(store_path.clone());
    let tmp_store_dir = tempfile::Builder::new()
        .prefix("jj-op-store-upgrade-")
        .tempdir_in(store_path.parent().unwrap())
        .unwrap();
    let tmp_store_path = tmp_store_dir.path().to_path_buf();

    // Find the current operation head(s) of the operation log. Because the hash is
    // based on the serialized format, it will be different after conversion, so
    // we need to rewrite these later.
    let op_heads_store_path = store_path.parent().unwrap().join("op_heads");
    let mut old_op_heads = HashSet::new();
    for entry in fs::read_dir(&op_heads_store_path)? {
        let basename = entry?.file_name();
        let op_id_str = basename.to_str().unwrap();
        if let Ok(op_id_bytes) = hex::decode(op_id_str) {
            old_op_heads.insert(OperationId::new(op_id_bytes));
        }
    }

    // Do a DFS to rewrite the operations
    let new_store = ThriftOpStore::init(tmp_store_path.clone());
    let mut converted: HashMap<OperationId, OperationId> = HashMap::new();
    // The DFS stack
    let mut to_convert = old_op_heads
        .iter()
        .map(|op_id| (op_id.clone(), old_store.read_operation(op_id).unwrap()))
        .collect_vec();
    while !to_convert.is_empty() {
        let (_, op) = to_convert.last().unwrap();
        let mut new_parent_ids: Vec<OperationId> = vec![];
        let mut new_to_convert = vec![];
        // Check which parents are already converted and which ones we need to rewrite
        // first
        for parent_id in &op.parents {
            if let Some(new_parent_id) = converted.get(parent_id) {
                new_parent_ids.push(new_parent_id.clone());
            } else {
                let parent_op = old_store.read_operation(parent_id).unwrap();
                new_to_convert.push((parent_id.clone(), parent_op));
            }
        }
        if new_to_convert.is_empty() {
            // If all parents have already been converted, remove this operation from the
            // stack and convert it
            let (old_op_id, mut old_op) = to_convert.pop().unwrap();
            old_op.parents = new_parent_ids;
            let old_view = old_store.read_view(&old_op.view_id).unwrap();
            let new_view_id = new_store.write_view(&old_view).unwrap();
            old_op.view_id = new_view_id;
            let new_op_id = new_store.write_operation(&old_op).unwrap();
            converted.insert(old_op_id, new_op_id);
        } else {
            to_convert.extend(new_to_convert);
        }
    }

    fs::write(tmp_store_path.join("thrift_store"), "")?;
    let backup_store_path = store_path.parent().unwrap().join("op_store_old");
    fs::rename(&store_path, backup_store_path)?;
    fs::rename(&tmp_store_path, &store_path)?;

    // Update the pointers to the head(s) of the operation log
    for old_op_head in old_op_heads {
        let new_op_head = converted.get(&old_op_head).unwrap().clone();
        fs::write(op_heads_store_path.join(new_op_head.hex()), "")?;
        fs::remove_file(op_heads_store_path.join(old_op_head.hex()))?;
    }

    // Update the pointers from operations to index files
    let index_operations_path = store_path
        .parent()
        .unwrap()
        .join("index")
        .join("operations");
    for entry in fs::read_dir(&index_operations_path)? {
        let basename = entry?.file_name();
        let op_id_str = basename.to_str().unwrap();
        if let Ok(op_id_bytes) = hex::decode(op_id_str) {
            let old_op_id = OperationId::new(op_id_bytes);
            // This should always succeed, but just skip it if it doesn't. We'll index
            // the commits on demand if we don't have an pointer to an index file.
            if let Some(new_op_id) = converted.get(&old_op_id) {
                fs::rename(
                    index_operations_path.join(basename),
                    index_operations_path.join(new_op_id.hex()),
                )?;
            }
        }
    }

    // Update the pointer to the last operation exported to Git
    let git_export_path = store_path.parent().unwrap().join("git_export_operation_id");
    if let Ok(op_id_string) = fs::read_to_string(&git_export_path) {
        if let Ok(op_id_bytes) = hex::decode(op_id_string) {
            let old_op_id = OperationId::new(op_id_bytes);
            let new_op_id = converted.get(&old_op_id).unwrap();
            fs::write(&git_export_path, new_op_id.hex())?;
        }
    }

    println!("Upgrade complete");
    Ok(())
}

impl SimpleOpStore {
    pub fn init(store_path: PathBuf) -> Self {
        fs::write(store_path.join("thrift_store"), "").unwrap();
        let delegate = ThriftOpStore::init(store_path);
        SimpleOpStore { delegate }
    }

    pub fn load(store_path: PathBuf) -> Self {
        if !store_path.join("thrift_store").exists() {
            upgrade_to_thrift(store_path.clone())
                .expect("Failed to upgrade operation log to Thrift format");
        }
        let delegate = ThriftOpStore::load(store_path);
        SimpleOpStore { delegate }
    }
}

impl OpStore for SimpleOpStore {
    fn read_view(&self, id: &ViewId) -> OpStoreResult<View> {
        self.delegate.read_view(id)
    }

    fn write_view(&self, view: &View) -> OpStoreResult<ViewId> {
        self.delegate.write_view(view)
    }

    fn read_operation(&self, id: &OperationId) -> OpStoreResult<Operation> {
        self.delegate.read_operation(id)
    }

    fn write_operation(&self, operation: &Operation) -> OpStoreResult<OperationId> {
        self.delegate.write_operation(operation)
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use maplit::{btreemap, hashmap, hashset};

    use super::*;
    use crate::backend::{CommitId, MillisSinceEpoch, Timestamp};
    use crate::content_hash::blake2b_hash;
    use crate::op_store::{BranchTarget, OperationMetadata, RefTarget, WorkspaceId};

    fn create_view() -> View {
        let head_id1 = CommitId::from_hex("aaa111");
        let head_id2 = CommitId::from_hex("aaa222");
        let public_head_id1 = CommitId::from_hex("bbb444");
        let public_head_id2 = CommitId::from_hex("bbb555");
        let branch_main_local_target = RefTarget::Normal(CommitId::from_hex("ccc111"));
        let branch_main_origin_target = RefTarget::Normal(CommitId::from_hex("ccc222"));
        let branch_deleted_origin_target = RefTarget::Normal(CommitId::from_hex("ccc333"));
        let tag_v1_target = RefTarget::Normal(CommitId::from_hex("ddd111"));
        let git_refs_main_target = RefTarget::Normal(CommitId::from_hex("fff111"));
        let git_refs_feature_target = RefTarget::Conflict {
            removes: vec![CommitId::from_hex("fff111")],
            adds: vec![CommitId::from_hex("fff222"), CommitId::from_hex("fff333")],
        };
        let default_wc_commit_id = CommitId::from_hex("abc111");
        let test_wc_commit_id = CommitId::from_hex("abc222");
        View {
            head_ids: hashset! {head_id1, head_id2},
            public_head_ids: hashset! {public_head_id1, public_head_id2},
            branches: btreemap! {
                "main".to_string() => BranchTarget {
                    local_target: Some(branch_main_local_target),
                    remote_targets: btreemap! {
                        "origin".to_string() => branch_main_origin_target,
                    }
                },
                "deleted".to_string() => BranchTarget {
                    local_target: None,
                    remote_targets: btreemap! {
                        "origin".to_string() => branch_deleted_origin_target,
                    }
                },
            },
            tags: btreemap! {
                "v1.0".to_string() => tag_v1_target,
            },
            git_refs: btreemap! {
                "refs/heads/main".to_string() => git_refs_main_target,
                "refs/heads/feature".to_string() => git_refs_feature_target
            },
            git_head: Some(CommitId::from_hex("fff111")),
            wc_commit_ids: hashmap! {
                WorkspaceId::default() => default_wc_commit_id,
                WorkspaceId::new("test".to_string()) => test_wc_commit_id,
            },
        }
    }

    fn create_operation() -> Operation {
        Operation {
            view_id: ViewId::from_hex("aaa111"),
            parents: vec![
                OperationId::from_hex("bbb111"),
                OperationId::from_hex("bbb222"),
            ],
            metadata: OperationMetadata {
                start_time: Timestamp {
                    timestamp: MillisSinceEpoch(123456789),
                    tz_offset: 3600,
                },
                end_time: Timestamp {
                    timestamp: MillisSinceEpoch(123456800),
                    tz_offset: 3600,
                },
                description: "check out foo".to_string(),
                hostname: "some.host.example.com".to_string(),
                username: "someone".to_string(),
                tags: hashmap! {
                    "key1".to_string() => "value1".to_string(),
                    "key2".to_string() => "value2".to_string(),
                },
            },
        }
    }

    #[test]
    fn test_hash_view() {
        // Test exact output so we detect regressions in compatibility
        assert_snapshot!(
            ViewId::new(blake2b_hash(&create_view()).to_vec()).hex(),
            @"2a026b6a091219a3d8ca43d822984cf9be0c53438225d76a5ba5e6d3724fab15104579fb08fa949977c4357b1806d240bef28d958cbcd7d786962ac88c15df31"
        );
    }

    #[test]
    fn test_hash_operation() {
        // Test exact output so we detect regressions in compatibility
        assert_snapshot!(
            OperationId::new(blake2b_hash(&create_operation()).to_vec()).hex(),
            @"3ec986c29ff8eb808ea8f6325d6307cea75ef02987536c8e4645406aba51afc8e229957a6e855170d77a66098c58912309323f5e0b32760caa2b59dc84d45fcf"
        );
    }

    #[test]
    fn test_read_write_view() {
        let temp_dir = testutils::new_temp_dir();
        let store = SimpleOpStore::init(temp_dir.path().to_owned());
        let view = create_view();
        let view_id = store.write_view(&view).unwrap();
        let read_view = store.read_view(&view_id).unwrap();
        assert_eq!(read_view, view);
    }

    #[test]
    fn test_read_write_operation() {
        let temp_dir = testutils::new_temp_dir();
        let store = SimpleOpStore::init(temp_dir.path().to_owned());
        let operation = create_operation();
        let op_id = store.write_operation(&operation).unwrap();
        let read_operation = store.read_operation(&op_id).unwrap();
        assert_eq!(read_operation, operation);
    }
}
