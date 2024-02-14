// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

#![allow(missing_docs)]

use std::cmp::Ordering;
use std::fmt::{Debug, Error, Formatter};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::op_store;
use crate::op_store::{OpStore, OpStoreResult, OperationId, ViewId};
use crate::view::View;

#[derive(Clone)]
pub struct Operation {
    op_store: Arc<dyn OpStore>,
    id: OperationId,
    data: op_store::Operation,
}

impl Debug for Operation {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.debug_struct("Operation").field("id", &self.id).finish()
    }
}

impl PartialEq for Operation {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Operation {}

impl Ord for Operation {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl PartialOrd for Operation {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for Operation {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}

impl Operation {
    pub fn new(op_store: Arc<dyn OpStore>, id: OperationId, data: op_store::Operation) -> Self {
        Operation { op_store, id, data }
    }

    pub fn op_store(&self) -> Arc<dyn OpStore> {
        self.op_store.clone()
    }

    pub fn id(&self) -> &OperationId {
        &self.id
    }

    pub fn view_id(&self) -> &ViewId {
        &self.data.view_id
    }

    pub fn parent_ids(&self) -> &[OperationId] {
        &self.data.parents
    }

    pub fn parents(&self) -> impl ExactSizeIterator<Item = OpStoreResult<Operation>> + '_ {
        let op_store = &self.op_store;
        self.data.parents.iter().map(|parent_id| {
            let data = op_store.read_operation(parent_id)?;
            Ok(Operation::new(op_store.clone(), parent_id.clone(), data))
        })
    }

    pub fn view(&self) -> OpStoreResult<View> {
        let data = self.op_store.read_view(&self.data.view_id)?;
        Ok(View::new(data))
    }

    pub fn store_operation(&self) -> &op_store::Operation {
        &self.data
    }
}
