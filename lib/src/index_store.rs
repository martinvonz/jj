// Copyright 2021 The Jujutsu Authors
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
use std::sync::Arc;

use thiserror::Error;

use crate::default_index_store::{MutableIndex, ReadonlyIndex};
use crate::op_store::OperationId;
use crate::operation::Operation;
use crate::store::Store;

#[derive(Debug, Error)]
pub enum IndexWriteError {
    #[error("{0}")]
    Other(String),
}

pub trait IndexStore: Send + Sync + Debug {
    fn name(&self) -> &str;

    fn get_index_at_op(&self, op: &Operation, store: &Arc<Store>) -> Arc<ReadonlyIndex>;

    fn write_index(
        &self,
        index: MutableIndex,
        op_id: &OperationId,
    ) -> Result<Arc<ReadonlyIndex>, IndexWriteError>;
}
