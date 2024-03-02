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

#![allow(missing_docs)]

use std::sync::Arc;

use itertools::Itertools as _;

use crate::backend::Timestamp;
use crate::index::ReadonlyIndex;
use crate::op_heads_store::OpHeadsStore;
use crate::op_store::OperationMetadata;
use crate::operation::Operation;
use crate::repo::{MutableRepo, ReadonlyRepo, Repo, RepoLoader, RepoLoaderError};
use crate::settings::UserSettings;
use crate::view::View;
use crate::{dag_walk, op_store};

/// An in-memory representation of a repo and any changes being made to it.
///
/// Within the scope of a transaction, changes to the repository are made
/// in-memory to `mut_repo` and published to the repo backend when
/// [`Transaction::commit`] is called. When a transaction is committed, it
/// becomes atomically visible as an Operation in the op log that represents the
/// transaction itself, and as a View that represents the state of the repo
/// after the transaction. This is similar to how a Commit represents a change
/// to the contents of the repository and a Tree represents the repository's
/// contents after the change. See the documentation for [`op_store::Operation`]
/// and [`op_store::View`] for more information.
pub struct Transaction {
    mut_repo: MutableRepo,
    parent_ops: Vec<Operation>,
    op_metadata: OperationMetadata,
    end_time: Option<Timestamp>,
}

impl Transaction {
    pub fn new(mut_repo: MutableRepo, user_settings: &UserSettings) -> Transaction {
        let parent_ops = vec![mut_repo.base_repo().operation().clone()];
        let op_metadata = create_op_metadata(user_settings, "".to_string(), false);
        let end_time = user_settings.operation_timestamp();
        Transaction {
            mut_repo,
            parent_ops,
            op_metadata,
            end_time,
        }
    }

    pub fn base_repo(&self) -> &Arc<ReadonlyRepo> {
        self.mut_repo.base_repo()
    }

    pub fn set_tag(&mut self, key: String, value: String) {
        self.op_metadata.tags.insert(key, value);
    }

    pub fn repo(&self) -> &MutableRepo {
        &self.mut_repo
    }

    pub fn mut_repo(&mut self) -> &mut MutableRepo {
        &mut self.mut_repo
    }

    pub fn merge_operation(&mut self, other_op: Operation) -> Result<(), RepoLoaderError> {
        let ancestor_op = dag_walk::closest_common_node_ok(
            self.parent_ops.iter().cloned().map(Ok),
            [Ok(other_op.clone())],
            |op: &Operation| op.id().clone(),
            |op: &Operation| op.parents().collect_vec(),
        )?
        .unwrap();
        let repo_loader = self.base_repo().loader();
        let base_repo = repo_loader.load_at(&ancestor_op)?;
        let other_repo = repo_loader.load_at(&other_op)?;
        self.parent_ops.push(other_op);
        let merged_repo = self.mut_repo();
        merged_repo.merge(&base_repo, &other_repo);
        Ok(())
    }

    pub fn set_is_snapshot(&mut self, is_snapshot: bool) {
        self.op_metadata.is_snapshot = is_snapshot;
    }

    /// Writes the transaction to the operation store and publishes it.
    pub fn commit(self, description: impl Into<String>) -> Arc<ReadonlyRepo> {
        self.write(description).publish()
    }

    /// Writes the transaction to the operation store, but does not publish it.
    /// That means that a repo can be loaded at the operation, but the
    /// operation will not be seen when loading the repo at head.
    pub fn write(mut self, description: impl Into<String>) -> UnpublishedOperation {
        let mut_repo = self.mut_repo;
        // TODO: Should we instead just do the rebasing here if necessary?
        assert!(
            !mut_repo.has_rewrites(),
            "BUG: Descendants have not been rebased after the last rewrites."
        );
        let base_repo = mut_repo.base_repo().clone();
        let (mut_index, view) = mut_repo.consume();

        let view_id = base_repo.op_store().write_view(view.store_view()).unwrap();
        self.op_metadata.description = description.into();
        self.op_metadata.end_time = self.end_time.unwrap_or_else(Timestamp::now);
        let parents = self.parent_ops.iter().map(|op| op.id().clone()).collect();
        let store_operation = op_store::Operation {
            view_id,
            parents,
            metadata: self.op_metadata,
        };
        let new_op_id = base_repo
            .op_store()
            .write_operation(&store_operation)
            .unwrap();
        let operation = Operation::new(base_repo.op_store().clone(), new_op_id, store_operation);

        let index = base_repo
            .index_store()
            .write_index(mut_index, operation.id())
            .unwrap();
        UnpublishedOperation::new(&base_repo.loader(), operation, view, index)
    }
}

pub fn create_op_metadata(
    user_settings: &UserSettings,
    description: String,
    is_snapshot: bool,
) -> OperationMetadata {
    let start_time = user_settings
        .operation_timestamp()
        .unwrap_or_else(Timestamp::now);
    let end_time = start_time.clone();
    let hostname = user_settings.operation_hostname();
    let username = user_settings.operation_username();
    OperationMetadata {
        start_time,
        end_time,
        description,
        hostname,
        username,
        is_snapshot,
        tags: Default::default(),
    }
}

/// An Operation which has been written to the operation store but not
/// published. The repo can be loaded at an unpublished Operation, but the
/// Operation will not be visible in the op log if the repo is loaded at head.
///
/// Either [`Self::publish`] or [`Self::leave_unpublished`] must be called to
/// finish the operation.
#[must_use = "Either publish() or leave_unpublished() must be called to finish the operation."]
pub struct UnpublishedOperation {
    op_heads_store: Arc<dyn OpHeadsStore>,
    repo: Arc<ReadonlyRepo>,
}

impl UnpublishedOperation {
    fn new(
        repo_loader: &RepoLoader,
        operation: Operation,
        view: View,
        index: Box<dyn ReadonlyIndex>,
    ) -> Self {
        UnpublishedOperation {
            op_heads_store: repo_loader.op_heads_store().clone(),
            repo: repo_loader.create_from(operation, view, index),
        }
    }

    pub fn operation(&self) -> &Operation {
        self.repo.operation()
    }

    pub fn publish(self) -> Arc<ReadonlyRepo> {
        let _lock = self.op_heads_store.lock();
        self.op_heads_store
            .update_op_heads(self.operation().parent_ids(), self.operation().id());
        self.repo
    }

    pub fn leave_unpublished(self) -> Arc<ReadonlyRepo> {
        self.repo
    }
}
