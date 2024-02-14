// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

#![allow(missing_docs)]

use std::sync::Arc;

use itertools::Itertools as _;

use crate::backend::Timestamp;
use crate::index::ReadonlyIndex;
use crate::op_store::OperationMetadata;
use crate::operation::Operation;
use crate::repo::{MutableRepo, ReadonlyRepo, Repo, RepoLoader, RepoLoaderError};
use crate::settings::UserSettings;
use crate::view::View;
use crate::{dag_walk, op_store};

pub struct Transaction {
    mut_repo: MutableRepo,
    parent_ops: Vec<Operation>,
    op_metadata: OperationMetadata,
    end_time: Option<Timestamp>,
}

impl Transaction {
    pub fn new(mut_repo: MutableRepo, user_settings: &UserSettings) -> Transaction {
        let parent_ops = vec![mut_repo.base_repo().operation().clone()];
        let op_metadata = create_op_metadata(user_settings, "".to_string());
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
        {
            let _lock = self.repo_loader.op_heads_store().lock();
            self.repo_loader
                .op_heads_store()
                .update_op_heads(data.operation.parent_ids(), data.operation.id());
        }
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
