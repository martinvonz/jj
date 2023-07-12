// Copyright 2021 The Jujutsu Authors
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

use crate::backend::CommitId;
use crate::conflicts::Conflict;
use crate::index::Index;
use crate::merge::trivial_merge;
use crate::op_store::{BranchTarget, RefTarget, RefTargetExt as _};

pub fn merge_ref_targets(
    index: &dyn Index,
    left: Option<&RefTarget>,
    base: Option<&RefTarget>,
    right: Option<&RefTarget>,
) -> Option<RefTarget> {
    if let Some(resolved) = trivial_merge(&[base], &[left, right]) {
        return resolved.cloned();
    }

    let conflict = Conflict::new(
        vec![ref_target_to_conflict(base)],
        vec![ref_target_to_conflict(left), ref_target_to_conflict(right)],
    )
    .flatten()
    .simplify();

    // TODO: switch to conflict.is_resolved()
    if conflict.as_resolved().is_some() {
        conflict_to_ref_target(conflict)
    } else {
        let conflict = merge_ref_targets_non_trivial(index, conflict);
        conflict_to_ref_target(conflict)
    }
}

fn conflict_to_ref_target(conflict: Conflict<Option<CommitId>>) -> Option<RefTarget> {
    match conflict.as_resolved() {
        Some(Some(id)) => Some(RefTarget::Normal(id.clone())),
        Some(None) => None, // Deleted ref
        None => {
            let (removes, adds) = conflict.into_legacy_form();
            Some(RefTarget::Conflict { removes, adds })
        }
    }
}

// TODO: Make RefTarget store or be aliased to Conflict<Option<CommitId>>.
// Since new conflict type can represent a deleted/absent ref, we might have
// to replace Option<RefTarget> with it. Map API might be a bit trickier.
fn ref_target_to_conflict(maybe_target: Option<&RefTarget>) -> Conflict<Option<CommitId>> {
    if let Some(target) = maybe_target {
        Conflict::from_legacy_form(target.removed_ids().cloned(), target.added_ids().cloned())
    } else {
        Conflict::resolved(None) // Deleted or absent ref
    }
}

fn merge_ref_targets_non_trivial(
    index: &dyn Index,
    conflict: Conflict<Option<CommitId>>,
) -> Conflict<Option<CommitId>> {
    let (mut removes, mut adds) = conflict.take();
    while let Some((remove_index, add_index)) = find_pair_to_remove(index, &removes, &adds) {
        removes.remove(remove_index);
        adds.remove(add_index);
    }
    Conflict::new(removes, adds)
}

fn find_pair_to_remove(
    index: &dyn Index,
    removes: &[Option<CommitId>],
    adds: &[Option<CommitId>],
) -> Option<(usize, usize)> {
    // If a "remove" is an ancestor of two different "adds" and one of the
    // "adds" is an ancestor of the other, then pick the descendant.
    for (add_index1, add1) in adds.iter().enumerate() {
        for (add_index2, add2) in adds.iter().enumerate().skip(add_index1 + 1) {
            // TODO: Instead of relying on the list order, maybe ((add1, add2), remove)
            // combination should be somehow weighted?
            let (add_index, add_id) = match (add1, add2) {
                (Some(id1), Some(id2)) if id1 == id2 => (add_index1, id1),
                (Some(id1), Some(id2)) if index.is_ancestor(id1, id2) => (add_index1, id1),
                (Some(id1), Some(id2)) if index.is_ancestor(id2, id1) => (add_index2, id2),
                _ => continue,
            };
            if let Some(remove_index) = removes.iter().position(|remove| match remove {
                Some(id) => index.is_ancestor(id, add_id),
                None => true, // Absent ref can be considered a root
            }) {
                return Some((remove_index, add_index));
            }
        }
    }

    None
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct BranchPushUpdate {
    pub old_target: Option<CommitId>,
    pub new_target: Option<CommitId>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum BranchPushAction {
    Update(BranchPushUpdate),
    AlreadyMatches,
    LocalConflicted,
    RemoteConflicted,
}

/// Figure out what changes (if any) need to be made to the remote when pushing
/// this branch.
pub fn classify_branch_push_action(
    branch_target: &BranchTarget,
    remote_name: &str,
) -> BranchPushAction {
    let local_target = branch_target.local_target.as_ref();
    let remote_target = branch_target.remote_targets.get(remote_name);
    if local_target == remote_target {
        BranchPushAction::AlreadyMatches
    } else if local_target.is_conflict() {
        BranchPushAction::LocalConflicted
    } else if remote_target.is_conflict() {
        BranchPushAction::RemoteConflicted
    } else {
        BranchPushAction::Update(BranchPushUpdate {
            old_target: remote_target.as_normal().cloned(),
            new_target: local_target.as_normal().cloned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use maplit::btreemap;

    use super::*;
    use crate::backend::ObjectId;

    #[test]
    fn test_classify_branch_push_action_unchanged() {
        let commit_id1 = CommitId::from_hex("11");
        let branch = BranchTarget {
            local_target: RefTarget::normal(commit_id1.clone()),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::normal(commit_id1).unwrap(),
            },
        };
        assert_eq!(
            classify_branch_push_action(&branch, "origin"),
            BranchPushAction::AlreadyMatches
        );
    }

    #[test]
    fn test_classify_branch_push_action_added() {
        let commit_id1 = CommitId::from_hex("11");
        let branch = BranchTarget {
            local_target: RefTarget::normal(commit_id1.clone()),
            remote_targets: btreemap! {},
        };
        assert_eq!(
            classify_branch_push_action(&branch, "origin"),
            BranchPushAction::Update(BranchPushUpdate {
                old_target: None,
                new_target: Some(commit_id1),
            })
        );
    }

    #[test]
    fn test_classify_branch_push_action_removed() {
        let commit_id1 = CommitId::from_hex("11");
        let branch = BranchTarget {
            local_target: RefTarget::absent(),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::normal(commit_id1.clone()).unwrap(),
            },
        };
        assert_eq!(
            classify_branch_push_action(&branch, "origin"),
            BranchPushAction::Update(BranchPushUpdate {
                old_target: Some(commit_id1),
                new_target: None,
            })
        );
    }

    #[test]
    fn test_classify_branch_push_action_updated() {
        let commit_id1 = CommitId::from_hex("11");
        let commit_id2 = CommitId::from_hex("22");
        let branch = BranchTarget {
            local_target: RefTarget::normal(commit_id2.clone()),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::normal(commit_id1.clone()).unwrap(),
            },
        };
        assert_eq!(
            classify_branch_push_action(&branch, "origin"),
            BranchPushAction::Update(BranchPushUpdate {
                old_target: Some(commit_id1),
                new_target: Some(commit_id2),
            })
        );
    }

    #[test]
    fn test_classify_branch_push_action_local_conflicted() {
        let commit_id1 = CommitId::from_hex("11");
        let commit_id2 = CommitId::from_hex("22");
        let branch = BranchTarget {
            local_target: RefTarget::from_legacy_form([], [commit_id1.clone(), commit_id2]),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::normal(commit_id1).unwrap(),
            },
        };
        assert_eq!(
            classify_branch_push_action(&branch, "origin"),
            BranchPushAction::LocalConflicted
        );
    }

    #[test]
    fn test_classify_branch_push_action_remote_conflicted() {
        let commit_id1 = CommitId::from_hex("11");
        let commit_id2 = CommitId::from_hex("22");
        let branch = BranchTarget {
            local_target: RefTarget::normal(commit_id1.clone()),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::from_legacy_form(
                    [],
                    [commit_id1, commit_id2],
                ).unwrap(),
            },
        };
        assert_eq!(
            classify_branch_push_action(&branch, "origin"),
            BranchPushAction::RemoteConflicted
        );
    }
}
