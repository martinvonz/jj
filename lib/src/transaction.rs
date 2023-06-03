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

use std::sync::Arc;

use crate::backend::Timestamp;
use crate::dag_walk::closest_common_node;
use crate::index::ReadonlyIndex;
use crate::op_store;
use crate::op_store::OperationMetadata;
use crate::operation::Operation;
use crate::repo::{MutableRepo, ReadonlyRepo, Repo, RepoLoader};
use crate::settings::UserSettings;
use crate::view::View;

pub struct Transaction {
    mut_repo: MutableRepo,
    parent_ops: Vec<Operation>,
    op_metadata: OperationMetadata,
    end_time: Option<Timestamp>,
}

impl Transaction {
    pub fn new(
        mut_repo: MutableRepo,
        user_settings: &UserSettings,
        description: &str,
    ) -> Transaction {
        let parent_ops = vec![mut_repo.base_repo().operation().clone()];
        let op_metadata = create_op_metadata(user_settings, description.to_string());
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

    pub fn set_description(&mut self, description: &str) {
        self.op_metadata.description = description.to_string();
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

    pub fn merge_operation(&mut self, other_op: Operation) {
        let ancestor_op = closest_common_node(
            self.parent_ops.clone(),
            vec![other_op.clone()],
            |op: &Operation| op.id().clone(),
            |op: &Operation| op.parents(),
        )
        .unwrap();
        let repo_loader = self.base_repo().loader();
        let base_repo = repo_loader.load_at(&ancestor_op);
        let other_repo = repo_loader.load_at(&other_op);
        self.parent_ops.push(other_op);
        let merged_repo = self.mut_repo();
        merged_repo.merge(&base_repo, &other_repo);
    }

    /// Writes the transaction to the operation store and publishes it.
    pub fn commit(self) -> Arc<ReadonlyRepo> {
        self.write().publish()
    }

    /// Writes the transaction to the operation store, but does not publish it.
    /// That means that a repo can be loaded at the operation, but the
    /// operation will not be seen when loading the repo at head.
    pub fn write(mut self) -> UnpublishedOperation {
        let mut_repo = self.mut_repo;
        // TODO: Should we instead just do the rebasing here if necessary?
        assert!(
            !mut_repo.has_rewrites(),
            "BUG: Descendants have not been rebased after the last rewrites."
        );
        let base_repo = mut_repo.base_repo().clone();
        let (mut_index, view) = mut_repo.consume();

        let view_id = base_repo.op_store().write_view(view.store_view()).unwrap();
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
        UnpublishedOperation::new(base_repo.loader(), operation, view, index)
    }
}

pub fn create_op_metadata(user_settings: &UserSettings, description: String) -> OperationMetadata {
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
        tags: Default::default(),
    }
}

struct NewRepoData {
    operation: Operation,
    view: View,
    index: Box<dyn ReadonlyIndex>,
}

pub struct UnpublishedOperation {
    repo_loader: RepoLoader,
    data: Option<NewRepoData>,
    closed: bool,
}

impl UnpublishedOperation {
    fn new(
        repo_loader: RepoLoader,
        operation: Operation,
        view: View,
        index: Box<dyn ReadonlyIndex>,
    ) -> Self {
        let data = Some(NewRepoData {
            operation,
            view,
            index,
        });
        UnpublishedOperation {
            repo_loader,
            data,
            closed: false,
        }
    }

    pub fn operation(&self) -> &Operation {
        &self.data.as_ref().unwrap().operation
    }

    pub fn publish(mut self) -> Arc<ReadonlyRepo> {
        let data = self.data.take().unwrap();
        self.repo_loader
            .op_heads_store()
            .lock()
            .promote_new_op(&data.operation);
        let repo = self
            .repo_loader
            .create_from(data.operation, data.view, data.index);
        self.closed = true;
        repo
    }

    pub fn leave_unpublished(mut self) -> Arc<ReadonlyRepo> {
        let data = self.data.take().unwrap();
        let repo = self
            .repo_loader
            .create_from(data.operation, data.view, data.index);
        self.closed = true;
        repo
    }
}

impl Drop for UnpublishedOperation {
    fn drop(&mut self) {
        if !self.closed && !std::thread::panicking() {
            eprintln!("BUG: UnpublishedOperation was dropped without being closed.");
        }
    }
}
