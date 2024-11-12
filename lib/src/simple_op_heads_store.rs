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

use std::any::Any;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use crate::lock::FileLock;
use crate::object_id::ObjectId;
use crate::op_heads_store::OpHeadsStore;
use crate::op_heads_store::OpHeadsStoreError;
use crate::op_heads_store::OpHeadsStoreLock;
use crate::op_store::OperationId;

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
    pub fn name() -> &'static str {
        "simple_op_heads_store"
    }

    pub fn init(dir: &Path) -> Self {
        let op_heads_dir = dir.join("heads");
        fs::create_dir(&op_heads_dir).unwrap();
        Self { dir: op_heads_dir }
    }

    pub fn load(dir: &Path) -> Self {
        let op_heads_dir = dir.join("heads");
        Self { dir: op_heads_dir }
    }

    fn add_op_head(&self, id: &OperationId) -> io::Result<()> {
        std::fs::write(self.dir.join(id.hex()), "")
    }

    fn remove_op_head(&self, id: &OperationId) -> io::Result<()> {
        std::fs::remove_file(self.dir.join(id.hex())).or_else(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                // It's fine if the old head was not found. It probably means
                // that we're on a distributed file system where the locking
                // doesn't work. We'll probably end up with two current
                // heads. We'll detect that next time we load the view.
                Ok(())
            } else {
                Err(err)
            }
        })
    }
}

struct SimpleOpHeadsStoreLock {
    _lock: FileLock,
}

impl OpHeadsStoreLock for SimpleOpHeadsStoreLock {}

impl OpHeadsStore for SimpleOpHeadsStore {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        Self::name()
    }

    fn update_op_heads(
        &self,
        old_ids: &[OperationId],
        new_id: &OperationId,
    ) -> Result<(), OpHeadsStoreError> {
        assert!(!old_ids.contains(new_id));
        self.add_op_head(new_id)
            .map_err(|err| OpHeadsStoreError::Write {
                new_op_id: new_id.clone(),
                source: err.into(),
            })?;
        for old_id in old_ids {
            self.remove_op_head(old_id)
                .map_err(|err| OpHeadsStoreError::Write {
                    new_op_id: new_id.clone(),
                    source: err.into(),
                })?;
        }
        Ok(())
    }

    fn get_op_heads(&self) -> Result<Vec<OperationId>, OpHeadsStoreError> {
        let mut op_heads = vec![];
        for op_head_entry in
            std::fs::read_dir(&self.dir).map_err(|err| OpHeadsStoreError::Read(err.into()))?
        {
            let op_head_file_name = op_head_entry
                .map_err(|err| OpHeadsStoreError::Read(err.into()))?
                .file_name();
            let op_head_file_name = op_head_file_name.to_str().ok_or_else(|| {
                OpHeadsStoreError::Read(
                    format!("Non-utf8 in op head file name: {op_head_file_name:?}").into(),
                )
            })?;
            if let Ok(op_head) = hex::decode(op_head_file_name) {
                op_heads.push(OperationId::new(op_head));
            }
        }
        Ok(op_heads)
    }

    fn lock(&self) -> Result<Box<dyn OpHeadsStoreLock + '_>, OpHeadsStoreError> {
        let lock = FileLock::lock(self.dir.join("lock"))
            .map_err(|err| OpHeadsStoreError::Lock(err.into()))?;
        Ok(Box::new(SimpleOpHeadsStoreLock { _lock: lock }))
    }
}
