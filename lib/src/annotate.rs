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
//! repository.
//!
//! TODO: Add support for different blame layers with a trait in the future.
//! Like commit metadata and more.

use std::collections::HashMap;

use pollster::FutureExt;
use thiserror::Error;

use crate::backend::BackendError;
use crate::backend::CommitId;
use crate::commit::Commit;
use crate::conflicts::materialize_tree_value;
use crate::conflicts::MaterializedTreeValue;
use crate::diff::Diff;
use crate::diff::DiffHunk;
use crate::fileset::FilesetExpression;
use crate::graph::GraphEdge;
use crate::graph::GraphEdgeType;
use crate::merged_tree::MergedTree;
use crate::repo::Repo;
use crate::repo_path::RepoPath;
use crate::revset::RevsetEvaluationError;
use crate::revset::RevsetExpression;
use crate::revset::RevsetFilterPredicate;
use crate::store::Store;

/// Various errors that can arise from annotation
#[derive(Debug, Error)]
pub enum AnnotateError {
    /// the requested file path was not found
    #[error("Unable to locate file: {0}")]
    FileNotFound(String),
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
    pub file_annotations: Vec<(CommitId, Vec<u8>)>,
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
        if let Some(old_map) = self.local_line_map.get_mut(old_commit_id) {
            if let Some(removed_original_line_number) = old_map.remove(&old_local_line_number) {
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
        }
    }

    /// Once we've looked at all parents of a commit, any leftover lines must be
    /// original to the current commit, so we save this information in
    /// original_line_map.
    fn drain_remaining_for_commit_id(&mut self, commit_id: &CommitId) {
        self.file_cache.remove(commit_id);
        if let Some(remaining_lines) = self.local_line_map.remove(commit_id) {
            for (_, original_line_number) in remaining_lines {
                self.original_line_map
                    .insert(original_line_number, commit_id.clone());
            }
        }
    }

    fn convert_to_results(self, original_contents: &[u8]) -> AnnotateResults {
        let mut result_lines = Vec::new();
        original_contents
            .split_inclusive(|b| *b == b'\n')
            .enumerate()
            .for_each(|(idx, line)| {
                result_lines.push((
                    self.original_line_map.get(&idx).unwrap().clone(),
                    line.to_owned(),
                ));
            });
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
        tree: &MergedTree,
    ) -> Result<(), AnnotateError> {
        if self.file_cache.contains_key(commit_id) {
            return Ok(());
        }

        if let Some(file_contents) = get_file_contents(store, file_path, tree)? {
            self.file_cache.insert(commit_id.clone(), file_contents);
        }

        Ok(())
    }
}

/// Get line by line annotations for a specific file path in the repo.
pub fn get_annotation_for_file(
    repo: &dyn Repo,
    starting_commit: &Commit,
    file_path: &RepoPath,
) -> Result<AnnotateResults, AnnotateError> {
    if let Some(original_contents) =
        get_file_contents(starting_commit.store(), file_path, &starting_commit.tree()?)?
    {
        let num_lines = original_contents.split_inclusive(|b| *b == b'\n').count();
        let mut partial_results = PartialResults::new(starting_commit.id(), num_lines);

        process_commits(
            repo,
            starting_commit.id(),
            &mut partial_results,
            file_path,
            num_lines,
        )?;

        Ok(partial_results.convert_to_results(&original_contents))
    } else {
        Err(AnnotateError::FileNotFound(
            file_path.as_internal_file_string().to_string(),
        ))
    }
}

/// Starting at the starting commit, compute changes at that commit, updating
/// the mappings. So long as there are mappings left in local_line_map, we
/// continue. Once local_line_map is empty, we've found sources for each line
/// and exit.
fn process_commits(
    repo: &dyn Repo,
    starting_commit_id: &CommitId,
    results: &mut PartialResults,
    file_name: &RepoPath,
    num_lines: usize,
) -> Result<(), AnnotateError> {
    let predicate = RevsetFilterPredicate::File(FilesetExpression::file_path(file_name.to_owned()));
    let revset = RevsetExpression::commit(starting_commit_id.clone())
        .union(
            &RevsetExpression::commit(starting_commit_id.clone())
                .ancestors()
                .filtered(predicate),
        )
        .evaluate_programmatic(repo)
        .map_err(|e| match e {
            RevsetEvaluationError::StoreError(backend_error) => AnnotateError::from(backend_error),
            RevsetEvaluationError::Other(_) => {
                panic!("Unable to evaluate internal revset")
            }
        })?;

    for (cid, edge_list) in revset.iter_graph() {
        let current_commit = repo.store().get_commit(&cid)?;
        process_commit(results, repo, file_name, &current_commit, &edge_list)?;
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
    repo: &dyn Repo,
    file_name: &RepoPath,
    current_commit: &Commit,
    edges: &Vec<GraphEdge<CommitId>>,
) -> Result<(), AnnotateError> {
    for parent_edge in edges {
        if parent_edge.edge_type != GraphEdgeType::Missing {
            let parent_commit = repo.store().get_commit(&parent_edge.target)?;
            process_files_in_commits(
                results,
                repo.store(),
                file_name,
                current_commit,
                &parent_commit,
            )?;
        }
    }
    results.drain_remaining_for_commit_id(current_commit.id());

    Ok(())
}

/// For two versions of the same file, for all the lines in common, overwrite
/// the new mapping in the results for the new commit. Let's say I have
/// a file in commit A and commit B. We know that according to local_line_map,
/// in commit A, line 3 corresponds to line 7 of the original file. Now, line 3
/// in Commit A corresponds to line 6 in commit B. Then, we update
/// local_line_map to say that "Commit B line 6 goes to line 7 of the original
/// file". We repeat this for all lines in common in the two commits. For 2
/// identical files, we bulk replace all mappings from commit A to commit B in
/// local_line_map
fn process_files_in_commits(
    results: &mut PartialResults,
    store: &Store,
    file_name: &RepoPath,
    current_commit: &Commit,
    parent_commit: &Commit,
) -> Result<(), AnnotateError> {
    results.load_file_into_cache(
        store,
        current_commit.id(),
        file_name,
        &current_commit.tree()?,
    )?;
    results.load_file_into_cache(store, parent_commit.id(), file_name, &parent_commit.tree()?)?;

    let current_contents = results.file_cache.get(current_commit.id()).unwrap();
    let parent_contents = results.file_cache.get(parent_commit.id()).unwrap();

    let same_lines = get_same_line_map(current_contents, parent_contents);
    for (current_line_no, parent_line_no) in same_lines {
        results.forward_to_new_commit(
            current_commit.id(),
            current_line_no,
            parent_commit.id(),
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
                for _ in common_string.split_inclusive(|b| *b == b'\n') {
                    result_map.insert(current_line_counter, parent_line_counter);
                    current_line_counter += 1;
                    parent_line_counter += 1;
                }
            }
            DiffHunk::Different(outputs) => {
                let current_output = outputs[0];
                let parent_output = outputs[1];
                if !current_output.is_empty() {
                    for _ in current_output.split_inclusive(|b| *b == b'\n') {
                        current_line_counter += 1;
                    }
                }
                if !parent_output.is_empty() {
                    for _ in parent_output.split_inclusive(|b| *b == b'\n') {
                        parent_line_counter += 1;
                    }
                }
            }
        }
    }

    result_map
}

fn get_file_contents(
    store: &Store,
    path: &RepoPath,
    tree: &MergedTree,
) -> Result<Option<Vec<u8>>, AnnotateError> {
    let file_value = tree.path_value(path)?;
    if file_value.is_absent() {
        Ok(None)
    } else {
        let effective_file_value = materialize_tree_value(store, path, file_value).block_on()?;
        match effective_file_value {
            MaterializedTreeValue::File { mut reader, id, .. } => {
                let mut file_contents = Vec::new();
                let _ =
                    reader
                        .read_to_end(&mut file_contents)
                        .map_err(|e| BackendError::ReadFile {
                            path: path.to_owned(),
                            id,
                            source: Box::new(e),
                        });
                Ok(Some(file_contents))
            }
            MaterializedTreeValue::Conflict { contents, .. } => Ok(Some(contents)),
            _ => Ok(None),
        }
    }
}
