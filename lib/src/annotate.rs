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
use crate::revset::RevsetExpression;
use crate::revset::RevsetFilterPredicate;
use crate::store::Store;

/// Annotation results for a specific file
pub struct AnnotateResults {
    /// An array of annotation results ordered by line.
    /// For each value in the array, the commit_id is the commit id of the
    /// originator of the line and the string is the actual line itself (without
    /// newline terminators). The vector is ordered by appearance in the
    /// file
    pub file_annotations: Vec<(CommitId, Vec<u8>)>,
}

/// A map from commits to line mappings.
/// Namely, for a given commit A, the value is the mapping of lines in the file
/// at commit A to line numbers in the original file
type CommitLineMap = HashMap<CommitId, HashMap<usize, usize>>;

/// Memoizes the file contents for a given version to save time
type FileCache = HashMap<CommitId, Vec<u8>>;

/// A map from line numbers in the original file to the commit that originated
/// that line
type OriginalLineMap = HashMap<usize, CommitId>;

fn get_initial_commit_line_map(commit_id: &CommitId, num_lines: usize) -> CommitLineMap {
    let mut starting_commit_map = HashMap::new();
    for i in 0..num_lines {
        starting_commit_map.insert(i, i);
    }

    let mut starting_line_map = HashMap::new();
    starting_line_map.insert(commit_id.clone(), starting_commit_map);
    starting_line_map
}

/// Once we've looked at all parents of a commit, any leftover lines must be
/// original to the current commit, so we save this information in
/// original_line_map.
fn mark_lines_from_original(
    original_line_map: &mut OriginalLineMap,
    commit_id: &CommitId,
    commit_lines: HashMap<usize, usize>,
) {
    for (_, original_line_number) in commit_lines {
        original_line_map.insert(original_line_number, commit_id.clone());
    }
}

/// Takes in an original line map and the original contents and annotates each
/// line according to the contents of the provided OriginalLineMap
fn convert_to_results(
    original_line_map: OriginalLineMap,
    original_contents: &[u8],
) -> AnnotateResults {
    let result_lines = original_contents
        .split_inclusive(|b| *b == b'\n')
        .enumerate()
        .map(|(idx, line)| {
            (
                original_line_map.get(&idx).unwrap().clone(),
                line.to_owned(),
            )
        })
        .collect();
    AnnotateResults {
        file_annotations: result_lines,
    }
}

/// loads a given file into the cache under a specific commit id.
/// If there is already a file for a given commit, it is a no-op.
fn load_file_into_cache(
    file_cache: &mut FileCache,
    store: &Store,
    commit_id: &CommitId,
    file_path: &RepoPath,
    tree: &MergedTree,
) -> Result<(), BackendError> {
    if file_cache.contains_key(commit_id) {
        return Ok(());
    }

    let file_contents = get_file_contents(store, file_path, tree)?;
    file_cache.insert(commit_id.clone(), file_contents);

    Ok(())
}

/// Get line by line annotations for a specific file path in the repo.
/// If the file is not found, returns empty results.
pub fn get_annotation_for_file(
    repo: &dyn Repo,
    starting_commit: &Commit,
    file_path: &RepoPath,
) -> Result<AnnotateResults, BackendError> {
    let original_contents =
        get_file_contents(starting_commit.store(), file_path, &starting_commit.tree()?)?;
    let num_lines = original_contents.split_inclusive(|b| *b == b'\n').count();
    let mut file_cache = HashMap::new();
    file_cache.insert(starting_commit.id().clone(), original_contents.clone());

    let original_line_map =
        process_commits(repo, file_cache, starting_commit.id(), file_path, num_lines)?;

    Ok(convert_to_results(original_line_map, &original_contents))
}

/// Starting at the starting commit, compute changes at that commit relative to
/// it's direct parents, updating the mappings as we go. We return the final
/// original line map that represents where each line of the original came from.
fn process_commits(
    repo: &dyn Repo,
    mut file_cache: FileCache,
    starting_commit_id: &CommitId,
    file_name: &RepoPath,
    num_lines: usize,
) -> Result<OriginalLineMap, BackendError> {
    let predicate = RevsetFilterPredicate::File(FilesetExpression::file_path(file_name.to_owned()));
    let revset = RevsetExpression::commit(starting_commit_id.clone())
        .union(
            &RevsetExpression::commit(starting_commit_id.clone())
                .ancestors()
                .filtered(predicate),
        )
        .evaluate_programmatic(repo)
        .map_err(|e| e.expect_backend_error())?;
    let mut commit_line_map = get_initial_commit_line_map(starting_commit_id, num_lines);
    let mut original_line_map = HashMap::new();

    for (cid, edge_list) in revset.iter_graph() {
        let current_commit = repo.store().get_commit(&cid)?;
        process_commit(
            repo,
            file_name,
            &mut original_line_map,
            &mut commit_line_map,
            &current_commit,
            &mut file_cache,
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
/// After iterating through all the parents, any leftover lines unmapped means
/// that those lines are original in the current commit. In that case,
/// original_line_map is updated for the leftover lines.
/// We return the lines that are the same in the child commit and
/// any parent. Namely, if line x is found in parent Y, we record the mapping
/// that parent Y has line x. The line mappings for all parents are returned
/// along with any lines originated in the current commit
fn process_commit(
    repo: &dyn Repo,
    file_name: &RepoPath,
    original_line_map: &mut HashMap<usize, CommitId>,
    commit_line_map: &mut CommitLineMap,
    current_commit: &Commit,
    file_cache: &mut FileCache,
    edges: &[GraphEdge<CommitId>],
) -> Result<(), BackendError> {
    if let Some(mut current_commit_line_map) = commit_line_map.remove(current_commit.id()) {
        for parent_edge in edges {
            if parent_edge.edge_type != GraphEdgeType::Missing {
                let parent_commit = repo.store().get_commit(&parent_edge.target)?;
                let same_line_map = process_files_in_commits(
                    repo.store(),
                    file_name,
                    file_cache,
                    current_commit,
                    &parent_commit,
                )?;

                let parent_commit_line_map = commit_line_map
                    .entry(parent_commit.id().clone())
                    .or_default();

                for (current_line_number, parent_line_number) in same_line_map {
                    if let Some(original_line_number) =
                        current_commit_line_map.remove(&current_line_number)
                    {
                        // forward current line to parent commit since it is in common
                        parent_commit_line_map.insert(parent_line_number, original_line_number);
                    }
                }
                if parent_commit_line_map.is_empty() {
                    commit_line_map.remove(parent_commit.id());
                }
            }
        }
        if !current_commit_line_map.is_empty() {
            mark_lines_from_original(
                original_line_map,
                current_commit.id(),
                current_commit_line_map,
            );
        }
        let _ = file_cache.remove(current_commit.id());
    }

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
    store: &Store,
    file_name: &RepoPath,
    file_cache: &mut FileCache,
    current_commit: &Commit,
    parent_commit: &Commit,
) -> Result<HashMap<usize, usize>, BackendError> {
    load_file_into_cache(
        file_cache,
        store,
        current_commit.id(),
        file_name,
        &current_commit.tree()?,
    )?;
    load_file_into_cache(
        file_cache,
        store,
        parent_commit.id(),
        file_name,
        &parent_commit.tree()?,
    )?;

    let current_contents = file_cache.get(current_commit.id()).unwrap();
    let parent_contents = file_cache.get(parent_commit.id()).unwrap();

    Ok(get_same_line_map(current_contents, parent_contents))
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
