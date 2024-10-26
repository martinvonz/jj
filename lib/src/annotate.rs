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

use std::collections::hash_map;
use std::collections::HashMap;

use bstr::BString;
use pollster::FutureExt;

use crate::backend::BackendError;
use crate::backend::CommitId;
use crate::commit::Commit;
use crate::conflicts::materialize_merge_result;
use crate::conflicts::materialize_tree_value;
use crate::conflicts::MaterializedTreeValue;
use crate::diff::Diff;
use crate::diff::DiffHunkKind;
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

/// Annotation results for a specific file
#[derive(Clone, Debug)]
pub struct AnnotateResults {
    /// An array of annotation results ordered by line.
    /// For each value in the array, the commit_id is the commit id of the
    /// originator of the line and the string is the actual line itself (without
    /// newline terminators). The vector is ordered by appearance in the
    /// file
    pub file_annotations: Vec<(CommitId, BString)>,
}

/// A map from commits to file line mappings and contents.
type CommitSourceMap = HashMap<CommitId, Source>;

/// Line mapping and file content at a certain commit.
#[derive(Clone, Debug)]
struct Source {
    /// Mapping of line numbers in the file at the current commit to the
    /// original file.
    line_map: HashMap<usize, usize>,
    /// File content at the current commit.
    text: BString,
}

impl Source {
    fn load(commit: &Commit, file_path: &RepoPath) -> Result<Self, BackendError> {
        let tree = commit.tree()?;
        let text = get_file_contents(commit.store(), file_path, &tree)?;
        Ok(Source {
            line_map: HashMap::new(),
            text: text.into(),
        })
    }

    fn fill_line_map(&mut self) {
        let lines = self.text.split_inclusive(|b| *b == b'\n');
        self.line_map = lines.enumerate().map(|(i, _)| (i, i)).collect();
    }
}

/// A map from line numbers in the original file to the commit that originated
/// that line
type OriginalLineMap = HashMap<usize, CommitId>;

/// Takes in an original line map and the original contents and annotates each
/// line according to the contents of the provided OriginalLineMap
fn convert_to_results(
    original_line_map: OriginalLineMap,
    original_contents: &[u8],
) -> AnnotateResults {
    let file_annotations = original_contents
        .split_inclusive(|b| *b == b'\n')
        .enumerate()
        .map(|(idx, line)| (original_line_map.get(&idx).unwrap().clone(), line.into()))
        .collect();
    AnnotateResults { file_annotations }
}

/// Get line by line annotations for a specific file path in the repo.
/// If the file is not found, returns empty results.
pub fn get_annotation_for_file(
    repo: &dyn Repo,
    starting_commit: &Commit,
    file_path: &RepoPath,
) -> Result<AnnotateResults, RevsetEvaluationError> {
    let mut source = Source::load(starting_commit, file_path)?;
    source.fill_line_map();
    let original_contents = source.text.clone();

    let original_line_map = process_commits(repo, starting_commit.id(), source, file_path)?;

    Ok(convert_to_results(original_line_map, &original_contents))
}

/// Starting at the starting commit, compute changes at that commit relative to
/// it's direct parents, updating the mappings as we go. We return the final
/// original line map that represents where each line of the original came from.
fn process_commits(
    repo: &dyn Repo,
    starting_commit_id: &CommitId,
    starting_source: Source,
    file_name: &RepoPath,
) -> Result<OriginalLineMap, RevsetEvaluationError> {
    let predicate = RevsetFilterPredicate::File(FilesetExpression::file_path(file_name.to_owned()));
    let revset = RevsetExpression::commit(starting_commit_id.clone())
        .union(
            &RevsetExpression::commit(starting_commit_id.clone())
                .ancestors()
                .filtered(predicate),
        )
        .evaluate_programmatic(repo)
        .map_err(|e| e.expect_backend_error())?;

    let num_lines = starting_source.line_map.len();
    let mut commit_source_map = HashMap::from([(starting_commit_id.clone(), starting_source)]);
    let mut original_line_map = HashMap::new();

    for node in revset.iter_graph() {
        let (commit_id, edge_list) = node?;
        process_commit(
            repo,
            file_name,
            &mut original_line_map,
            &mut commit_source_map,
            &commit_id,
            &edge_list,
        )?;
        if original_line_map.len() >= num_lines {
            break;
        }
    }
    Ok(original_line_map)
}

/// For a given commit, for each parent, we compare the version in the parent
/// tree with the current version, updating the mappings for any lines in
/// common. If the parent doesn't have the file, we skip it.
fn process_commit(
    repo: &dyn Repo,
    file_name: &RepoPath,
    original_line_map: &mut OriginalLineMap,
    commit_source_map: &mut CommitSourceMap,
    current_commit_id: &CommitId,
    edges: &[GraphEdge<CommitId>],
) -> Result<(), BackendError> {
    let Some(mut current_source) = commit_source_map.remove(current_commit_id) else {
        return Ok(());
    };

    for parent_edge in edges {
        if parent_edge.edge_type == GraphEdgeType::Missing {
            continue;
        }
        let parent_commit_id = &parent_edge.target;
        let parent_source = match commit_source_map.entry(parent_commit_id.clone()) {
            hash_map::Entry::Occupied(entry) => entry.into_mut(),
            hash_map::Entry::Vacant(entry) => {
                let commit = repo.store().get_commit(entry.key())?;
                entry.insert(Source::load(&commit, file_name)?)
            }
        };

        // For two versions of the same file, for all the lines in common,
        // overwrite the new mapping in the results for the new commit. Let's
        // say I have a file in commit A and commit B. We know that according to
        // local line_map, in commit A, line 3 corresponds to line 7 of the
        // original file. Now, line 3 in Commit A corresponds to line 6 in
        // commit B. Then, we update local line_map to say that "Commit B line 6
        // goes to line 7 of the original file". We repeat this for all lines in
        // common in the two commits.
        let same_line_map = get_same_line_map(&current_source.text, &parent_source.text);
        for (current_line_number, parent_line_number) in same_line_map {
            let Some(original_line_number) = current_source.line_map.remove(&current_line_number)
            else {
                continue;
            };
            parent_source
                .line_map
                .insert(parent_line_number, original_line_number);
        }
        if parent_source.line_map.is_empty() {
            commit_source_map.remove(parent_commit_id);
        }
    }

    // Once we've looked at all parents of a commit, any leftover lines must be
    // original to the current commit, so we save this information in
    // original_line_map.
    for &original_line_number in current_source.line_map.values() {
        original_line_map.insert(original_line_number, current_commit_id.clone());
    }

    Ok(())
}

/// For two files, get a map of all lines in common (e.g. line 8 maps to line 9)
fn get_same_line_map(current_contents: &[u8], parent_contents: &[u8]) -> HashMap<usize, usize> {
    let mut result_map = HashMap::new();
    let diff = Diff::by_line([current_contents, parent_contents]);
    let mut current_line_counter: usize = 0;
    let mut parent_line_counter: usize = 0;
    for hunk in diff.hunks() {
        match hunk.kind {
            DiffHunkKind::Matching => {
                for _ in hunk.contents[0].split_inclusive(|b| *b == b'\n') {
                    result_map.insert(current_line_counter, parent_line_counter);
                    current_line_counter += 1;
                    parent_line_counter += 1;
                }
            }
            DiffHunkKind::Different => {
                let current_output = hunk.contents[0];
                let parent_output = hunk.contents[1];
                current_line_counter += current_output.split_inclusive(|b| *b == b'\n').count();
                parent_line_counter += parent_output.split_inclusive(|b| *b == b'\n').count();
            }
        }
    }

    result_map
}

fn get_file_contents(
    store: &Store,
    path: &RepoPath,
    tree: &MergedTree,
) -> Result<Vec<u8>, BackendError> {
    let file_value = tree.path_value(path)?;
    if file_value.is_absent() {
        Ok(Vec::new())
    } else {
        let effective_file_value = materialize_tree_value(store, path, file_value).block_on()?;
        match effective_file_value {
            MaterializedTreeValue::File { mut reader, id, .. } => {
                let mut file_contents = Vec::new();
                reader
                    .read_to_end(&mut file_contents)
                    .map_err(|e| BackendError::ReadFile {
                        path: path.to_owned(),
                        id,
                        source: Box::new(e),
                    })?;
                Ok(file_contents)
            }
            MaterializedTreeValue::FileConflict { id, contents, .. } => {
                let mut materialized_conflict_buffer = Vec::new();
                materialize_merge_result(&contents, &mut materialized_conflict_buffer).map_err(
                    |io_err| BackendError::ReadFile {
                        path: path.to_owned(),
                        source: Box::new(io_err),
                        id: id.first().clone().unwrap(),
                    },
                )?;
                Ok(materialized_conflict_buffer)
            }
            _ => Ok(Vec::new()),
        }
    }
}
