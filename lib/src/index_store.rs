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

use crate::index::ReadonlyIndex;
use crate::op_store::OperationId;
use crate::repo::ReadonlyRepo;
use std::path::PathBuf;
use std::sync::Arc;

pub struct IndexStore {
    dir: PathBuf,
}

impl IndexStore {
    pub fn init(dir: PathBuf) -> Self {
        std::fs::create_dir(dir.join("operations")).unwrap();
        IndexStore { dir }
    }

    pub fn reinit(&self) {
        std::fs::remove_dir_all(self.dir.join("operations")).unwrap();
        IndexStore::init(self.dir.clone());
    }

    pub fn load(dir: PathBuf) -> IndexStore {
        IndexStore { dir }
    }

    pub fn get_index_at_op(&self, repo: &ReadonlyRepo, op_id: OperationId) -> Arc<ReadonlyIndex> {
        let op_id_hex = op_id.hex();
        let op_id_file = self.dir.join("operations").join(&op_id_hex);
        if op_id_file.exists() {
            let op_id = OperationId(hex::decode(op_id_hex).unwrap());
            ReadonlyIndex::load_at_operation(self.dir.clone(), repo.store().hash_length(), &op_id)
                .unwrap()
        } else {
            let op = repo.view().as_view_ref().get_operation(&op_id).unwrap();
            ReadonlyIndex::index(repo.store(), self.dir.clone(), &op).unwrap()
        }
    }
}
