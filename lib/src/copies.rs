// Copyright 2024 The Jujutsu Authors
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

//! Code for working with copies and renames.

use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::executor::block_on_stream;
use futures::stream::BoxStream;
use futures::Stream;

use crate::backend::{BackendResult, CopyRecord};
use crate::merge::MergedTreeValue;
use crate::merged_tree::{MergedTree, TreeDiffStream};
use crate::repo_path::{RepoPath, RepoPathBuf};

/// A collection of CopyRecords.
#[derive(Default, Debug)]
pub struct CopyRecords {
    records: Vec<CopyRecord>,
    // Maps from `target` to the index of the target in `records`.  Conflicts
    // are excluded by keeping an out of range value.
    targets: HashMap<RepoPathBuf, usize>,
}

impl CopyRecords {
    /// Adds information about a stream of CopyRecords to `self`.  A target with
    /// multiple conflicts is discarded and treated as not having an origin.
    pub fn add_records(
        &mut self,
        stream: BoxStream<BackendResult<CopyRecord>>,
    ) -> BackendResult<()> {
        for record in block_on_stream(stream) {
            let r = record?;
            let value = self
                .targets
                .entry(r.target.clone())
                .or_insert(self.records.len());

            if *value != self.records.len() {
                // TODO: handle conflicts instead of ignoring both sides.
                *value = usize::MAX;
            }
            self.records.push(r);
        }
        Ok(())
    }

    /// Gets any copy record associated with a target path.
    pub fn for_target(&self, target: &RepoPath) -> Option<&CopyRecord> {
        self.targets.get(target).and_then(|&i| self.records.get(i))
    }

    /// Gets all copy records.
    pub fn iter(&self) -> impl Iterator<Item = &CopyRecord> + '_ {
        self.records.iter()
    }
}

/// Wraps a `TreeDiffStream`, adding support for copies and renames.
pub struct CopiesTreeDiffStream<'a> {
    inner: TreeDiffStream<'a>,
    source_tree: MergedTree,
    copy_records: &'a CopyRecords,
}

impl<'a> CopiesTreeDiffStream<'a> {
    /// Create a new diff stream with copy information.
    pub fn new(
        inner: TreeDiffStream<'a>,
        source_tree: MergedTree,
        copy_records: &'a CopyRecords,
    ) -> Self {
        Self {
            inner,
            source_tree,
            copy_records,
        }
    }
}

/// A `TreeDiffEntry` with copy information.
pub struct CopiesTreeDiffEntry {
    /// The source path.
    pub source: RepoPathBuf,
    /// The target path.
    pub target: RepoPathBuf,
    /// The resolved tree values if available.
    pub value: BackendResult<(MergedTreeValue, MergedTreeValue)>,
}

impl Stream for CopiesTreeDiffStream<'_> {
    type Item = CopiesTreeDiffEntry;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx).map(|option| {
            option.map(|diff_entry| {
                let Some(CopyRecord { source, .. }) =
                    self.copy_records.for_target(&diff_entry.target)
                else {
                    return CopiesTreeDiffEntry {
                        source: diff_entry.source,
                        target: diff_entry.target,
                        value: diff_entry.value,
                    };
                };

                CopiesTreeDiffEntry {
                    source: source.clone(),
                    target: diff_entry.target,
                    value: diff_entry.value.and_then(|(_, target_value)| {
                        self.source_tree
                            .path_value(source)
                            .map(|source_value| (source_value, target_value))
                    }),
                }
            })
        })
    }
}
