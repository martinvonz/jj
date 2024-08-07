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

//! Methods that allow annotation (attribution and blame) for a file in a
//! repository

use std::collections::HashMap;
use std::io::BufRead;

use thiserror::Error;

use crate::backend::{BackendError, BackendResult, CommitId, FileId, TreeValue};
use crate::diff::{Diff, DiffHunk};
use crate::fileset::FilesetExpression;
use crate::graph::{GraphEdge, GraphEdgeType};
use crate::merged_tree::MergedTree;
use crate::object_id::ObjectId;
use crate::repo::{ReadonlyRepo, Repo};
use crate::repo_path::RepoPath;
use crate::revset::{RevsetExpression, RevsetFilterPredicate};
use crate::store::Store;

/// Various errors that can arise from annotation
#[derive(Debug, Error)]
pub enum AnnotateError {
    /// the requested file path was not found
    #[error("Unable to locate file: {0}")]
    FileNotFound(String),
    /// the file type is incorrect. Usually a directory was given but a regular
    /// file is required
    #[error("File {0} must be a regular file, not a directory")]
    UnsupportedFileType(String),
    /// the file is in a conflicted state and can therefore not be annotated
    /// properly
    #[error("File {0} is conflicted at commit: {0}")]
    Conflicted(String, String),
    /// pass-through of uncaught backend errors
    #[error(transparent)]
    BackendError(#[from] BackendError),
}

/// Annotation results for a specific file
pub struct AnnotateResults {
    /// An array of annotation results ordered by line.
    /// For each value in the array, the commit_id is the commit id of the
    /// originator of the line and the string is the actual line itself (without
    /// newline terminators). The vector is ordered by appearance in the
    /// file
    pub file_annotations: Vec<(CommitId, String)>,
}

/// A note on implementation:
/// This structure represents the results along the way.
/// We first start at the original commit, for each commit, we compare the file
/// to the version in each parent. We only look at lines in common. For each
/// line in common, we add it to local_line_map according to how the lines match
/// up. If, we discover a line that is not in common with any parent commit, we
/// know that the current commit originated that line and we add it to
/// original_line_map.
/// We then proceed to walk through the graph, until we've found commits for
/// each line (local_line_map is empty when this happens)
struct PartialResults {
    /// A mapping from line_number in the original file to most recent commit
    /// that changed it.
    original_line_map: HashMap<usize, CommitId>,
    /// CommitId -> (line_number in CommitId -> line_number in the original).
    /// This is a map for a given commit_id, returns a mapping of line numbers
    /// in the file version at commit_id to the original version.
    /// For example, Commit 123 contains a map {(1, 1), (2, 3)} which means line
    /// 1 at 123 goes to the original line 1 and line 2 at 123 goes to line 3 at
    /// the original
    local_line_map: HashMap<CommitId, HashMap<usize, usize>>,
    /// A store of previously seen files
    file_cache: HashMap<CommitId, Vec<u8>>,
}

impl PartialResults {
    fn new(starting_commit_id: &CommitId, num_lines: usize) -> Self {
        let mut starting_map = HashMap::new();
        for i in 0..num_lines {
            starting_map.insert(i, i);
        }
        let mut results = PartialResults {
            original_line_map: HashMap::new(),
            local_line_map: HashMap::new(),
            file_cache: HashMap::new(),
        };
        results
            .local_line_map
            .insert(starting_commit_id.clone(), starting_map);
        results
    }

    /// Take a line mapping from an old commit and move it to a new commit.
    /// For example, if we figure out that line 2 in commit A maps to line 7 in
    /// the original, and line 3 in commit B maps to line 2 in commit A, we
    /// update the mapping so line 3 maps to line 7 in the original.
    fn forward_to_new_commit(
        &mut self,
        old_commit_id: &CommitId,
        old_local_line_number: usize,
        new_commit_id: &CommitId,
        new_local_line_number: usize,
    ) {
        let old_map = self.local_line_map.get_mut(old_commit_id);
        if old_map.is_none() {
            return;
        }
        let old_map = old_map.unwrap();
        let removed_original_line_number = old_map.remove(&old_local_line_number);
        if removed_original_line_number.is_none() {
            return;
        }
        let removed_original_line_number = removed_original_line_number.unwrap();
        if self.local_line_map.contains_key(new_commit_id) {
            self.local_line_map
                .get_mut(new_commit_id)
                .unwrap()
                .insert(new_local_line_number, removed_original_line_number);
        } else {
            let mut new_map = HashMap::new();
            new_map.insert(new_local_line_number, removed_original_line_number);
            self.local_line_map.insert(new_commit_id.clone(), new_map);
        }
    }

    /// Used for two commits with the same file contents. We wholesale move all
    /// mappings from the old commit to the new commit.
    fn swap_full_commit_id(&mut self, old_commit_id: &CommitId, new_commit_id: &CommitId) {
        let old_commit_map = self.local_line_map.remove(old_commit_id);
        if old_commit_map.is_none() {
            return;
        }
        let old_commit_map = old_commit_map.unwrap();
        self.local_line_map
            .insert(new_commit_id.clone(), old_commit_map);
    }

    /// Once we've looked at all parents of a commit, any leftover lines must be
    /// original to the current commit, so we save this information in
    /// original_line_map.
    fn drain_remaining_for_commit_id(&mut self, commit_id: &CommitId) {
        self.file_cache.remove(commit_id);
        let remaining_lines = self.local_line_map.remove(commit_id);
        if remaining_lines.is_none() {
            return;
        }
        let remaining_lines = remaining_lines.unwrap();
        for (_, original_line_number) in remaining_lines {
            self.original_line_map
                .insert(original_line_number, commit_id.clone());
        }
    }

    fn convert_to_results(self, original_contents: &[u8]) -> AnnotateResults {
        let original_content_lines: Vec<String> =
            original_contents.lines().map(|s| s.unwrap()).collect();
        let mut result_lines = Vec::new();
        for (idx, line) in original_content_lines.into_iter().enumerate() {
            result_lines.push((self.original_line_map.get(&idx).unwrap().clone(), line));
        }
        AnnotateResults {
            file_annotations: result_lines,
        }
    }

    /// loads a given file into the cache under a specific commit id.
    /// If there is already a file for a given commit, it is a no-op.
    fn load_file_into_cache(
        &mut self,
        store: &Store,
        commit_id: &CommitId,
        file_path: &RepoPath,
        file_id: &FileId,
    ) -> BackendResult<()> {
        if self.file_cache.contains_key(commit_id) {
            return Ok(());
        }

        let file_contents = get_file_contents(store, file_path, file_id)?;
        self.file_cache.insert(commit_id.clone(), file_contents);

        Ok(())
    }

    /// retrieves a file from the cache for a specific commit. If the file isn't
    /// found, it panics. Make sure to call load_file_into_cache first.
    fn get_file_from_cache(&self, commit_id: &CommitId) -> &[u8] {
        self.file_cache.get(commit_id).unwrap()
    }
}

/// Get line by line annotations for a specific file path in the repo.
pub fn get_annotation_for_file(
    file_path: &RepoPath,
    repo: &ReadonlyRepo,
    starting_commit_id: &CommitId,
) -> Result<AnnotateResults, AnnotateError> {
    let store = repo.store();

    let current_commit = store.get_commit(starting_commit_id)?;
    let current_tree = current_commit.tree()?;
    let original_file_id = get_file_id(current_commit.id(), &current_tree, file_path)?;
    if original_file_id.is_none() {
        return Err(AnnotateError::FileNotFound(
            file_path.as_internal_file_string().to_string(),
        ));
    }
    let original_contents = get_file_contents(repo.store(), file_path, &original_file_id.unwrap())?;
    let num_lines = original_contents.split_inclusive(|b| *b == b'\n').count();
    let mut partial_results = PartialResults::new(starting_commit_id, num_lines);

    process_commits(
        &mut partial_results,
        repo,
        current_commit.id(),
        file_path,
        num_lines,
    )?;

    Ok(partial_results.convert_to_results(&original_contents))
}

/// Starting at the starting commit, compute changes at that commit, updating
/// the mappings. So long as there are mappings left in local_line_map, we
/// continue. Once local_line_map is empty, we've found sources for each line
/// and exit.
fn process_commits(
    results: &mut PartialResults,
    repo: &ReadonlyRepo,
    starting_commit_id: &CommitId,
    file_name: &RepoPath,
    num_lines: usize,
) -> Result<(), AnnotateError> {
    let predicate = RevsetFilterPredicate::File(FilesetExpression::file_path(file_name.to_owned()));
    let revset = RevsetExpression::commit(starting_commit_id.clone())
        .ancestors()
        .filtered(predicate)
        .evaluate_programmatic(repo)
        .unwrap();
    let mut is_first = true;

    for (cid, edge_list) in revset.iter_graph() {
        if is_first {
            results.swap_full_commit_id(starting_commit_id, &cid);
            is_first = false;
        }
        process_commit(results, repo, file_name, &cid, &edge_list)?;
        if results.original_line_map.len() >= num_lines {
            break;
        }
    }
    Ok(())
}

/// For a given commit, for each parent, we compare the version in the parent
/// tree with the current version, updating the mappings for any lines in
/// common. If the parent doesn't have the file, we skip it.
/// After iterating through all the parents, any leftover lines unmapped means
/// that those lines are original in the current commit. In that case,
/// original_line_map is updated for the leftover lines.
fn process_commit(
    results: &mut PartialResults,
    repo: &ReadonlyRepo,
    file_name: &RepoPath,
    current_commit_id: &CommitId,
    edges: &Vec<GraphEdge<CommitId>>,
) -> Result<(), AnnotateError> {
    let current_tree = repo.store().get_commit(current_commit_id)?.tree()?;
    let current_file_id = get_file_id(current_commit_id, &current_tree, file_name)?.unwrap();

    for parent_edge in edges {
        if parent_edge.edge_type != GraphEdgeType::Missing {
            let parent_commit = repo.store().get_commit(&parent_edge.target)?;
            let parent_tree = parent_commit.tree()?;
            let parent_file_id = get_file_id(parent_commit.id(), &parent_tree, file_name)?;

            if let Some(pfid) = parent_file_id {
                process_file_ids(
                    results,
                    repo.store(),
                    file_name,
                    &current_file_id,
                    current_commit_id,
                    &pfid,
                    parent_commit.id(),
                )?;
            }
        }
    }
    results.drain_remaining_for_commit_id(current_commit_id);

    Ok(())
}

/// For two versions of the same file, for all the lines in common, overwrite
/// the new mapping in the results for the new commit. Meaning, Let's say I have
/// a file in commit A and commit B. We know that according to local_line_map,
/// in commit A, line 3 corresponds to line 7 of the original file. Now, line 3
/// in Commit A corresponds to line 6 in commit B. Then, we update
/// local_line_map to say that "Commit B line 6 goes to line 7 of the original
/// file". We repeat this for all lines in common in the two commits. For 2
/// identical files, we bulk replace all mappings from commit A to commit B in
/// local_line_map
fn process_file_ids(
    results: &mut PartialResults,
    store: &Store,
    file_name: &RepoPath,
    current_file_id: &FileId,
    current_commit_id: &CommitId,
    parent_file_id: &FileId,
    parent_commit_id: &CommitId,
) -> BackendResult<()> {
    if current_file_id == parent_file_id {
        results.swap_full_commit_id(current_commit_id, parent_commit_id);
        return Ok(());
    }

    results.load_file_into_cache(store, current_commit_id, file_name, current_file_id)?;
    results.load_file_into_cache(store, parent_commit_id, file_name, parent_file_id)?;

    let current_contents = results.get_file_from_cache(current_commit_id);
    let parent_contents = results.get_file_from_cache(parent_commit_id);

    let same_lines = get_same_line_map(current_contents, parent_contents);
    for (current_line_no, parent_line_no) in same_lines {
        results.forward_to_new_commit(
            current_commit_id,
            current_line_no,
            parent_commit_id,
            parent_line_no,
        );
    }
    Ok(())
}

/// For two files, get a map of all lines in common (e.g. line 8 maps to line 9)
fn get_same_line_map(current_contents: &[u8], parent_contents: &[u8]) -> HashMap<usize, usize> {
    let mut result_map = HashMap::new();
    let inputs = vec![current_contents, parent_contents];
    let diff = Diff::by_line(&inputs);
    let mut current_line_counter: usize = 0;
    let mut parent_line_counter: usize = 0;
    for hunk in diff.hunks() {
        match hunk {
            DiffHunk::Matching(common_string) => {
                for _ in common_string.lines() {
                    result_map.insert(current_line_counter, parent_line_counter);
                    current_line_counter += 1;
                    parent_line_counter += 1;
                }
            }
            DiffHunk::Different(outputs) => {
                let current_output = outputs[0];
                let parent_output = outputs[1];
                if !current_output.is_empty() {
                    for _ in current_output.lines() {
                        current_line_counter += 1;
                    }
                }
                if !parent_output.is_empty() {
                    for _ in parent_output.lines() {
                        parent_line_counter += 1;
                    }
                }
            }
        }
    }

    result_map
}

fn get_file_id(
    commit_id: &CommitId,
    tree: &MergedTree,
    file_name: &RepoPath,
) -> Result<Option<FileId>, AnnotateError> {
    let file_value = tree.path_value(file_name)?;
    if file_value.is_absent() {
        return Ok(None);
    }
    if !file_value.is_resolved() {
        return Err(AnnotateError::Conflicted(
            file_name.to_internal_dir_string(),
            commit_id.hex(),
        ));
    }

    let file_object = file_value.first().as_ref().unwrap();
    match file_object {
        TreeValue::File { id, .. } => Ok(Some(id.clone())),
        _ => Err(AnnotateError::UnsupportedFileType(
            file_name.to_internal_dir_string(),
        )),
    }
}

fn get_file_contents(store: &Store, path: &RepoPath, file_id: &FileId) -> BackendResult<Vec<u8>> {
    let mut reader = store.read_file(path, file_id)?;
    let mut contents: Vec<u8> = Vec::new();
    let err = reader.read_to_end(&mut contents);
    if let Err(e) = err {
        return Err(BackendError::ReadFile {
            path: path.to_owned(),
            id: file_id.to_owned(),
            source: Box::new(e),
        });
    }
    Ok(contents)
}
