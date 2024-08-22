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
use std::task::ready;
use std::task::Context;
use std::task::Poll;

use futures::Stream;

use crate::backend::BackendResult;
use crate::backend::CopyRecord;
use crate::merge::MergedTreeValue;
use crate::merged_tree::MergedTree;
use crate::merged_tree::TreeDiffStream;
use crate::repo_path::RepoPath;
use crate::repo_path::RepoPathBuf;

/// A collection of CopyRecords.
#[derive(Default, Debug)]
pub struct CopyRecords {
    records: Vec<CopyRecord>,
    // Maps from `source` or `target` to the index of the entry in `records`.
    // Conflicts are excluded by keeping an out of range value.
    sources: HashMap<RepoPathBuf, usize>,
    targets: HashMap<RepoPathBuf, usize>,
}

impl CopyRecords {
    /// Adds information about `CopyRecord`s to `self`. A target with multiple
    /// conflicts is discarded and treated as not having an origin.
    pub fn add_records(
        &mut self,
        copy_records: impl IntoIterator<Item = BackendResult<CopyRecord>>,
    ) -> BackendResult<()> {
        for record in copy_records {
            let r = record?;
            self.sources
                .entry(r.source.clone())
                // TODO: handle conflicts instead of ignoring both sides.
                .and_modify(|value| *value = usize::MAX)
                .or_insert(self.records.len());
            self.targets
                .entry(r.target.clone())
                // TODO: handle conflicts instead of ignoring both sides.
                .and_modify(|value| *value = usize::MAX)
                .or_insert(self.records.len());
            self.records.push(r);
        }
        Ok(())
    }

    /// Returns true if there are copy records associated with a source path.
    pub fn has_source(&self, source: &RepoPath) -> bool {
        self.sources.contains_key(source)
    }

    /// Gets any copy record associated with a source path.
    pub fn for_source(&self, source: &RepoPath) -> Option<&CopyRecord> {
        self.sources.get(source).and_then(|&i| self.records.get(i))
    }

    /// Returns true if there are copy records associated with a target path.
    pub fn has_target(&self, target: &RepoPath) -> bool {
        self.targets.contains_key(target)
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

/// Whether or not the source path was deleted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CopyOperation {
    /// The source path was not deleted.
    Copy,
    /// The source path was renamed to the destination.
    Rename,
}

/// A `TreeDiffEntry` with copy information.
pub struct CopiesTreeDiffEntry {
    /// The path.
    pub path: CopiesTreeDiffEntryPath,
    /// The resolved tree values if available.
    pub values: BackendResult<(MergedTreeValue, MergedTreeValue)>,
}

/// Path and copy information of `CopiesTreeDiffEntry`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopiesTreeDiffEntryPath {
    /// The source path and copy information if this is a copy or rename.
    pub source: Option<(RepoPathBuf, CopyOperation)>,
    /// The target path.
    pub target: RepoPathBuf,
}

impl CopiesTreeDiffEntryPath {
    /// The source path.
    pub fn source(&self) -> &RepoPath {
        self.source.as_ref().map_or(&self.target, |(path, _)| path)
    }

    /// The target path.
    pub fn target(&self) -> &RepoPath {
        &self.target
    }

    /// Whether this entry was copied or renamed from the source. Returns `None`
    /// if the path is unchanged.
    pub fn copy_operation(&self) -> Option<CopyOperation> {
        self.source.as_ref().map(|(_, op)| *op)
    }
}

/// Wraps a `TreeDiffStream`, adding support for copies and renames.
pub struct CopiesTreeDiffStream<'a> {
    inner: TreeDiffStream<'a>,
    source_tree: MergedTree,
    target_tree: MergedTree,
    copy_records: &'a CopyRecords,
}

impl<'a> CopiesTreeDiffStream<'a> {
    /// Create a new diff stream with copy information.
    pub fn new(
        inner: TreeDiffStream<'a>,
        source_tree: MergedTree,
        target_tree: MergedTree,
        copy_records: &'a CopyRecords,
    ) -> Self {
        Self {
            inner,
            source_tree,
            target_tree,
            copy_records,
        }
    }

    fn resolve_copy_source(
        &self,
        source: &RepoPath,
        values: BackendResult<(MergedTreeValue, MergedTreeValue)>,
    ) -> BackendResult<(CopyOperation, (MergedTreeValue, MergedTreeValue))> {
        let (_, target_value) = values?;
        let source_value = self.source_tree.path_value(source)?;
        // If the source path is deleted in the target tree, it's a rename.
        let copy_op = if self.target_tree.path_value(source)?.is_absent() {
            CopyOperation::Rename
        } else {
            CopyOperation::Copy
        };
        Ok((copy_op, (source_value, target_value)))
    }
}

impl Stream for CopiesTreeDiffStream<'_> {
    type Item = CopiesTreeDiffEntry;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        while let Some(diff_entry) = ready!(self.inner.as_mut().poll_next(cx)) {
            let Some(CopyRecord { source, .. }) = self.copy_records.for_target(&diff_entry.path)
            else {
                let target_deleted =
                    matches!(&diff_entry.values, Ok((_, target_value)) if target_value.is_absent());
                if target_deleted && self.copy_records.has_source(&diff_entry.path) {
                    // Skip the "delete" entry when there is a rename.
                    continue;
                }
                return Poll::Ready(Some(CopiesTreeDiffEntry {
                    path: CopiesTreeDiffEntryPath {
                        source: None,
                        target: diff_entry.path,
                    },
                    values: diff_entry.values,
                }));
            };

            let (copy_op, values) = match self.resolve_copy_source(source, diff_entry.values) {
                Ok((copy_op, values)) => (copy_op, Ok(values)),
                // Fall back to "copy" (= path still exists) if unknown.
                Err(err) => (CopyOperation::Copy, Err(err)),
            };
            return Poll::Ready(Some(CopiesTreeDiffEntry {
                path: CopiesTreeDiffEntryPath {
                    source: Some((source.clone(), copy_op)),
                    target: diff_entry.path,
                },
                values,
            }));
        }

        Poll::Ready(None)
    }
}
