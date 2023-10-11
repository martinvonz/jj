// Copyright 2023 The Jujutsu Authors
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

//! Defines the interface for the working copy. See `LocalWorkingCopy` for the
//! default local-disk implementation.

use std::any::Any;
use std::path::Path;

use crate::backend::MergedTreeId;
use crate::op_store::{OperationId, WorkspaceId};

/// The trait all working-copy implementations must implement.
pub trait WorkingCopy {
    /// Should return `self`. For down-casting purposes.
    fn as_any(&self) -> &dyn Any;

    /// The name/id of the implementation. Used for choosing the right
    /// implementation when loading a working copy.
    fn name(&self) -> &str;

    /// The working copy's root directory.
    fn path(&self) -> &Path;

    /// The working copy's workspace ID.
    fn workspace_id(&self) -> &WorkspaceId;

    /// The operation this working copy was most recently updated to.
    fn operation_id(&self) -> &OperationId;
}

/// A working copy that's being modified.
pub trait LockedWorkingCopy {
    /// Should return `self`. For down-casting purposes.
    fn as_any(&self) -> &dyn Any;

    /// The operation at the time the lock was taken
    fn old_operation_id(&self) -> &OperationId;

    /// The tree at the time the lock was taken
    fn old_tree_id(&self) -> &MergedTreeId;
}
