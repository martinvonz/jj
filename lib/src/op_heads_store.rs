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

use crate::op_store::{OpStore, OperationId};
use crate::operation::Operation;

pub enum OpHeads {
    /// There's a single latest operation. This is the normal case.
    Single(Operation),
    /// There are multiple latest operations, which means there has been
    /// concurrent operations. These need to be resolved.
    Unresolved {
        locked_op_heads: LockedOpHeads,
        op_heads: Vec<Operation>,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpHeadResolutionError {
    #[error("Operation log has no heads")]
    NoHeads,
}

pub trait LockedOpHeadsResolver {
    fn finish(&self, new_op: &Operation);
}

// Represents a mutually exclusive lock on the OpHeadsStore in local systems.
pub struct LockedOpHeads {
    resolver: Box<dyn LockedOpHeadsResolver>,
}

impl LockedOpHeads {
    pub fn new(resolver: Box<dyn LockedOpHeadsResolver>) -> Self {
        LockedOpHeads { resolver }
    }

    pub fn finish(self, new_op: &Operation) {
        self.resolver.finish(new_op);
    }
}

/// Manages the very set of current heads of the operation log.
///
/// Implementations should use Arc<> internally, as the lock() and
/// get_heads() return values which might outlive the original object. When Rust
/// makes it possible for a Trait method to reference &Arc<Self>, this can be
/// simplified.
pub trait OpHeadsStore: Send + Sync + Debug {
    fn add_op_head(&self, id: &OperationId);

    fn remove_op_head(&self, id: &OperationId);

    fn get_op_heads(&self) -> Vec<OperationId>;

    fn lock(&self) -> LockedOpHeads;

    fn get_heads(&self, op_store: &Arc<dyn OpStore>) -> Result<OpHeads, OpHeadResolutionError>;
}
