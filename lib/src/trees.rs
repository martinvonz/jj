// Copyright 2020 Google LLC
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

use crate::files;
use crate::files::MergeResult;
use crate::matchers::Matcher;
use crate::repo_path::{
    DirRepoPath, DirRepoPathComponent, FileRepoPath, FileRepoPathComponent, RepoPathJoin,
};
use crate::store::{Conflict, ConflictPart, StoreError, TreeId, TreeValue};
use crate::store_wrapper::StoreWrapper;
use crate::tree::Tree;
use std::cmp::Ordering;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Diff<T> {
    Modified(T, T),
    Added(T),
    Removed(T),
}

pub type TreeValueDiff<'a> = Diff<&'a TreeValue>;

fn diff_entries<'a, E>(
    tree1: &'a Tree,
    tree2: &'a Tree,
    callback: &mut impl FnMut(&'a str, TreeValueDiff<'a>) -> Result<(), E>,
) -> Result<(), E> {
    let mut it1 = tree1.entries_non_recursive();
    let mut it2 = tree2.entries_non_recursive();
    let mut entry1 = it1.next();
    let mut entry2 = it2.next();
    loop {
        let name: &'a str;
        let mut value_before: Option<&'a TreeValue> = None;
        let mut value_after: Option<&'a TreeValue> = None;
        match (&entry1, &entry2) {
            (Some(before), Some(after)) => {
                match before.name().cmp(after.name()) {
                    Ordering::Less => {
                        // entry removed
                        name = before.name();
                        value_before = Some(before.value());
                    }
                    Ordering::Greater => {
                        // entry added
                        name = after.name();
                        value_after = Some(after.value());
                    }
                    Ordering::Equal => {
                        // entry modified
                        name = before.name();
                        value_before = Some(before.value());
                        value_after = Some(after.value());
                    }
                }
            }
            (Some(before), None) => {
                // second iterator exhausted
                name = before.name();
                value_before = Some(before.value());
            }
            (None, Some(after)) => {
                // first iterator exhausted
                name = after.name();
                value_after = Some(after.value());
            }
            (None, None) => {
                // both iterators exhausted
                break;
            }
        }

        match (value_before, value_after) {
            (Some(before), Some(after)) => {
                if before != after {
                    callback(name, TreeValueDiff::Modified(before, after))?;
                }
                entry1 = it1.next();
                entry2 = it2.next();
            }
            (Some(before), None) => {
                callback(name, TreeValueDiff::Removed(before))?;
                entry1 = it1.next();
            }
            (None, Some(after)) => {
                callback(name, TreeValueDiff::Added(after))?;
                entry2 = it2.next();
            }
            (None, None) => {
                panic!("should have been handled above");
            }
        }
    }
    Ok(())
}

pub fn recursive_tree_diff<M>(
    root1: Tree,
    root2: Tree,
    matcher: &M,
    callback: &mut impl FnMut(&FileRepoPath, TreeValueDiff),
) where
    M: Matcher,
{
    internal_recursive_tree_diff(vec![(DirRepoPath::root(), root1, root2)], matcher, callback)
}

fn internal_recursive_tree_diff<M>(
    work: Vec<(DirRepoPath, Tree, Tree)>,
    _matcher: &M,
    callback: &mut impl FnMut(&FileRepoPath, TreeValueDiff),
) where
    M: Matcher,
{
    let mut new_work = Vec::new();
    // Diffs for which to invoke the callback after having visited subtrees. This is
    // used for making sure that when a directory gets replaced by a file, we
    // call the callback for the addition of the file after we call the callback
    // for removing files in the directory.
    let mut late_file_diffs = Vec::new();
    for (dir, tree1, tree2) in &work {
        diff_entries(tree1, tree2, &mut |name,
                                         diff: TreeValueDiff|
         -> Result<(), ()> {
            let file_path = dir.join(&FileRepoPathComponent::from(name));
            let subdir = DirRepoPathComponent::from(name);
            let subdir_path = dir.join(&subdir);
            // TODO: simplify this mess
            match diff {
                TreeValueDiff::Modified(TreeValue::Tree(id_before), TreeValue::Tree(id_after)) => {
                    new_work.push((
                        subdir_path,
                        tree1.known_sub_tree(&subdir, &id_before),
                        tree2.known_sub_tree(&subdir, &id_after),
                    ));
                }
                TreeValueDiff::Modified(TreeValue::Tree(id_before), file_after) => {
                    new_work.push((
                        subdir_path.clone(),
                        tree1.known_sub_tree(&subdir, &id_before),
                        Tree::null(tree2.store().clone(), subdir_path),
                    ));
                    late_file_diffs.push((file_path, TreeValueDiff::Added(file_after)));
                }
                TreeValueDiff::Modified(file_before, TreeValue::Tree(id_after)) => {
                    new_work.push((
                        subdir_path.clone(),
                        Tree::null(tree1.store().clone(), subdir_path),
                        tree2.known_sub_tree(&subdir, &id_after),
                    ));
                    callback(&file_path, TreeValueDiff::Removed(file_before));
                }
                TreeValueDiff::Modified(_, _) => {
                    callback(&file_path, diff);
                }
                TreeValueDiff::Added(TreeValue::Tree(id_after)) => {
                    new_work.push((
                        subdir_path.clone(),
                        Tree::null(tree1.store().clone(), subdir_path),
                        tree2.known_sub_tree(&subdir, &id_after),
                    ));
                }
                TreeValueDiff::Added(_) => {
                    callback(&file_path, diff);
                }
                TreeValueDiff::Removed(TreeValue::Tree(id_before)) => {
                    new_work.push((
                        subdir_path.clone(),
                        tree1.known_sub_tree(&subdir, &id_before),
                        Tree::null(tree2.store().clone(), subdir_path),
                    ));
                }
                TreeValueDiff::Removed(_) => {
                    callback(&file_path, diff);
                }
            };
            Ok(())
        })
        .unwrap(); // safe because the callback always returns Ok
    }
    if !new_work.is_empty() {
        internal_recursive_tree_diff(new_work, _matcher, callback)
    }
    for (file_path, diff) in late_file_diffs {
        callback(&file_path, diff);
    }
}

pub fn merge_trees(
    side1_tree: &Tree,
    base_tree: &Tree,
    side2_tree: &Tree,
) -> Result<TreeId, StoreError> {
    let store = base_tree.store().as_ref();
    let dir = base_tree.dir();
    assert_eq!(side1_tree.dir(), dir);
    assert_eq!(side2_tree.dir(), dir);

    if base_tree.id() == side1_tree.id() {
        return Ok(side2_tree.id().clone());
    }
    if base_tree.id() == side2_tree.id() || side1_tree.id() == side2_tree.id() {
        return Ok(side1_tree.id().clone());
    }

    // Start with a tree identical to side 1 and modify based on changes from base
    // to side 2.
    let mut new_tree = side1_tree.data().clone();
    diff_entries(base_tree, side2_tree, &mut |basename,
                                              diff|
     -> Result<(), StoreError> {
        let maybe_side1 = side1_tree.value(basename);
        let (maybe_base, maybe_side2) = match diff {
            TreeValueDiff::Modified(base, side2) => (Some(base), Some(side2)),
            TreeValueDiff::Added(side2) => (None, Some(side2)),
            TreeValueDiff::Removed(base) => (Some(base), None),
        };
        if maybe_side1 == maybe_base {
            // side 1 is unchanged: use the value from side 2
            match maybe_side2 {
                None => new_tree.remove(basename),
                Some(side2) => new_tree.set(basename.to_owned(), side2.clone()),
            };
        } else if maybe_side1 == maybe_side2 {
            // Both sides changed in the same way: new_tree already has the
            // value
        } else {
            // The two sides changed in different ways
            let new_value =
                merge_tree_value(store, dir, basename, maybe_base, maybe_side1, maybe_side2)?;
            match new_value {
                None => new_tree.remove(basename),
                Some(value) => new_tree.set(basename.to_owned(), value),
            }
        }
        Ok(())
    })?;
    store.write_tree(dir, &new_tree)
}

fn merge_tree_value(
    store: &StoreWrapper,
    dir: &DirRepoPath,
    basename: &str,
    maybe_base: Option<&TreeValue>,
    maybe_side1: Option<&TreeValue>,
    maybe_side2: Option<&TreeValue>,
) -> Result<Option<TreeValue>, StoreError> {
    // Resolve non-trivial conflicts:
    //   * resolve tree conflicts by recursing
    //   * try to resolve file conflicts by merging the file contents
    //   * leave other conflicts (e.g. file/dir conflicts, remove/modify conflicts)
    //     unresolved
    Ok(match (maybe_base, maybe_side1, maybe_side2) {
        (
            Some(TreeValue::Tree(base)),
            Some(TreeValue::Tree(side1)),
            Some(TreeValue::Tree(side2)),
        ) => {
            let subdir = dir.join(&DirRepoPathComponent::from(basename));
            let merged_tree_id = merge_trees(
                &store.get_tree(&subdir, &side1).unwrap(),
                &store.get_tree(&subdir, &base).unwrap(),
                &store.get_tree(&subdir, &side2).unwrap(),
            )?;
            if &merged_tree_id == store.empty_tree_id() {
                None
            } else {
                Some(TreeValue::Tree(merged_tree_id))
            }
        }
        _ => {
            let maybe_merged = match (maybe_base, maybe_side1, maybe_side2) {
                (
                    Some(TreeValue::Normal {
                        id: base_id,
                        executable: base_executable,
                    }),
                    Some(TreeValue::Normal {
                        id: side1_id,
                        executable: side1_executable,
                    }),
                    Some(TreeValue::Normal {
                        id: side2_id,
                        executable: side2_executable,
                    }),
                ) => {
                    let executable = if base_executable == side1_executable {
                        *side2_executable
                    } else if base_executable == side2_executable {
                        *side1_executable
                    } else {
                        assert_eq!(side1_executable, side2_executable);
                        *side1_executable
                    };

                    let filename = dir.join(&FileRepoPathComponent::from(basename));
                    let mut base_content = vec![];
                    store
                        .read_file(&filename, &base_id)?
                        .read_to_end(&mut base_content)?;
                    let mut side1_content = vec![];
                    store
                        .read_file(&filename, &side1_id)?
                        .read_to_end(&mut side1_content)?;
                    let mut side2_content = vec![];
                    store
                        .read_file(&filename, &side2_id)?
                        .read_to_end(&mut side2_content)?;

                    let merge_result = files::merge(&base_content, &side1_content, &side2_content);
                    match merge_result {
                        MergeResult::Resolved(merged_content) => {
                            let id = store.write_file(&filename, &mut merged_content.as_slice())?;
                            Some(TreeValue::Normal { id, executable })
                        }
                        MergeResult::Conflict(_) => None,
                    }
                }
                _ => None,
            };
            match maybe_merged {
                Some(merged) => Some(merged),
                None => {
                    let mut conflict = Conflict::default();
                    if let Some(base) = maybe_base {
                        conflict.removes.push(ConflictPart {
                            value: base.clone(),
                        });
                    }
                    if let Some(side1) = maybe_side1 {
                        conflict.adds.push(ConflictPart {
                            value: side1.clone(),
                        });
                    }
                    if let Some(side2) = maybe_side2 {
                        conflict.adds.push(ConflictPart {
                            value: side2.clone(),
                        });
                    }
                    simplify_conflict(store, &conflict)?
                }
            }
        }
    })
}

fn conflict_part_to_conflict(
    store: &StoreWrapper,
    part: &ConflictPart,
) -> Result<Conflict, StoreError> {
    match &part.value {
        TreeValue::Conflict(id) => {
            let conflict = store.read_conflict(id)?;
            Ok(conflict)
        }
        other => Ok(Conflict {
            removes: vec![],
            adds: vec![ConflictPart {
                value: other.clone(),
            }],
        }),
    }
}

fn simplify_conflict(
    store: &StoreWrapper,
    conflict: &Conflict,
) -> Result<Option<TreeValue>, StoreError> {
    // Important cases to simplify:
    //
    // D
    // |
    // B C
    // |/
    // A
    //
    // 1. rebase C to B, then back to A => there should be no conflict
    // 2. rebase C to B, then to D => the conflict should not mention B
    // 3. rebase B to C and D to B', then resolve the conflict in B' and rebase D'
    // on top =>    the conflict should be between B'', B, and D; it should not
    // mention the conflict in B'

    // Case 1 above:
    // After first rebase, the conflict is {+B-A+C}. After rebasing back,
    // the unsimplified conflict is {+A-B+{+B-A+C}}. Since the
    // inner conflict is positive, we can simply move it into the outer conflict. We
    // thus get {+A-B+B-A+C}, which we can then simplify to just C (because {+C} ==
    // C).
    //
    // Case 2 above:
    // After first rebase, the conflict is {+B-A+C}. After rebasing to D,
    // the unsimplified conflict is {+D-C+{+B-A+C}}. As in the
    // previous case, the inner conflict can be moved into the outer one. We then
    // get {+D-C+B-A+C}. That can be simplified to
    // {+D+B-A}, which is the desired conflict.
    //
    // Case 3 above:
    // TODO: describe this case

    // First expand any diffs with nested conflicts.
    let mut new_removes = vec![];
    let mut new_adds = vec![];
    for part in &conflict.adds {
        match part.value {
            TreeValue::Conflict(_) => {
                let conflict = conflict_part_to_conflict(&store, part)?;
                new_removes.extend_from_slice(&conflict.removes);
                new_adds.extend_from_slice(&conflict.adds);
            }
            _ => {
                new_adds.push(part.clone());
            }
        }
    }
    for part in &conflict.removes {
        match part.value {
            TreeValue::Conflict(_) => {
                let conflict = conflict_part_to_conflict(&store, part)?;
                new_removes.extend_from_slice(&conflict.adds);
                new_adds.extend_from_slice(&conflict.removes);
            }
            _ => {
                new_removes.push(part.clone());
            }
        }
    }

    // Remove pairs of entries that match in the removes and adds.
    let mut add_index = 0;
    while add_index < new_adds.len() {
        let add = &new_adds[add_index];
        add_index += 1;
        for (remove_index, remove) in new_removes.iter().enumerate() {
            if remove.value == add.value {
                new_removes.remove(remove_index);
                add_index -= 1;
                new_adds.remove(add_index);
                break;
            }
        }
    }

    // TODO: We should probably remove duplicate entries here too. So if we have
    // {+A+A}, that would become just {+A}. Similarly {+B-A+B} would be just
    // {+B-A}.

    if new_adds.is_empty() {
        // If there are no values to add, then the path doesn't exist (so return None to
        // indicate that).
        return Ok(None);
    }

    if new_removes.is_empty() && new_adds.len() == 1 {
        // A single add means that the current state is that state.
        return Ok(Some(new_adds[0].value.clone()));
    }

    let conflict_id = store.write_conflict(&Conflict {
        adds: new_adds,
        removes: new_removes,
    })?;
    Ok(Some(TreeValue::Conflict(conflict_id)))
}
