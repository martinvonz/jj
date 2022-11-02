// Copyright 2020 Google LLC
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

use std::fmt::Debug;
use std::path::PathBuf;

use crate::op_store::{OpStore, OpStoreResult, Operation, OperationId, View, ViewId};
use crate::proto_op_store::ProtoOpStore;

#[derive(Debug)]
pub struct SimpleOpStore {
    delegate: ProtoOpStore,
}

impl SimpleOpStore {
    pub fn init(store_path: PathBuf) -> Self {
        let delegate = ProtoOpStore::init(store_path);
        SimpleOpStore { delegate }
    }

    pub fn load(store_path: PathBuf) -> Self {
        let delegate = ProtoOpStore::load(store_path);
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
