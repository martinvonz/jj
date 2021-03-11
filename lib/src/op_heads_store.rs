// Copyright 2021 Google LLC
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

use crate::lock::FileLock;
use crate::op_store::OperationId;
use crate::operation::Operation;
use std::path::PathBuf;

/// Manages the very set of current heads of the operation log. The store is
/// simply a directory where each operation id is a file with that name (and no
/// content).
pub struct OpHeadsStore {
    dir: PathBuf,
}

impl OpHeadsStore {
    pub fn init(dir: PathBuf) -> Self {
        OpHeadsStore { dir }
    }

    pub fn load(dir: PathBuf) -> OpHeadsStore {
        OpHeadsStore { dir }
    }

    pub fn add_op_head(&self, id: &OperationId) {
        std::fs::write(self.dir.join(id.hex()), "").unwrap();
    }

    pub fn remove_op_head(&self, id: &OperationId) {
        // It's fine if the old head was not found. It probably means
        // that we're on a distributed file system where the locking
        // doesn't work. We'll probably end up with two current
        // heads. We'll detect that next time we load the view.
        std::fs::remove_file(self.dir.join(id.hex())).ok();
    }

    pub fn get_op_heads(&self) -> Vec<OperationId> {
        let mut op_heads = vec![];
        for op_head_entry in std::fs::read_dir(&self.dir).unwrap() {
            let op_head_file_name = op_head_entry.unwrap().file_name();
            let op_head_file_name = op_head_file_name.to_str().unwrap();
            if let Ok(op_head) = hex::decode(op_head_file_name) {
                op_heads.push(OperationId(op_head));
            }
        }
        op_heads
    }

    pub fn lock(&self) -> FileLock {
        FileLock::lock(self.dir.join("lock"))
    }

    pub fn update_op_heads(&self, op: &Operation) {
        let _op_heads_lock = self.lock();
        self.add_op_head(op.id());
        for old_parent_id in op.parent_ids() {
            self.remove_op_head(old_parent_id);
        }
    }
}
