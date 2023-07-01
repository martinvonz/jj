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

use crate::backend::CommitId;
use crate::index::Index;
use crate::merge::trivial_merge;
use crate::op_store::{BranchTarget, RefTarget};

pub fn merge_ref_targets(
    index: &dyn Index,
    left: Option<&RefTarget>,
    base: Option<&RefTarget>,
    right: Option<&RefTarget>,
) -> Option<RefTarget> {
    if let Some(resolved) = trivial_merge(&[base], &[left, right]) {
        return resolved.cloned();
    }

    let mut removes = vec![];
    let mut adds = vec![];
    if let Some(left) = left {
        removes.extend_from_slice(left.removes());
        adds.extend_from_slice(left.adds());
    }
    if let Some(base) = base {
        // Note that these are backwards (because the base is subtracted).
        removes.extend_from_slice(base.adds());
        adds.extend_from_slice(base.removes());
    }
    if let Some(right) = right {
        removes.extend_from_slice(right.removes());
        adds.extend_from_slice(right.adds());
    }

    while let Some((maybe_remove_index, add_index)) = find_pair_to_remove(index, &removes, &adds) {
        if let Some(remove_index) = maybe_remove_index {
            removes.remove(remove_index);
        }
        adds.remove(add_index);
    }

    if adds.is_empty() {
        None
    } else if adds.len() == 1 && removes.is_empty() {
        Some(RefTarget::Normal(adds[0].clone()))
    } else {
        Some(RefTarget::Conflict { removes, adds })
    }
}

fn find_pair_to_remove(
    index: &dyn Index,
    removes: &[CommitId],
    adds: &[CommitId],
) -> Option<(Option<usize>, usize)> {
    // Removes pairs of matching adds and removes.
    for (remove_index, remove) in removes.iter().enumerate() {
        for (add_index, add) in adds.iter().enumerate() {
            if add == remove {
                return Some((Some(remove_index), add_index));
            }
        }
    }

    // If a "remove" is an ancestor of two different "adds" and one of the
    // "adds" is an ancestor of the other, then pick the descendant.
    for (add_index1, add1) in adds.iter().enumerate() {
        for (add_index2, add2) in adds.iter().enumerate().skip(add_index1 + 1) {
            let first_add_is_ancestor;
            if add1 == add2 || index.is_ancestor(add1, add2) {
                first_add_is_ancestor = true;
            } else if index.is_ancestor(add2, add1) {
                first_add_is_ancestor = false;
            } else {
                continue;
            }
            if removes.is_empty() {
                if first_add_is_ancestor {
                    return Some((None, add_index1));
                } else {
                    return Some((None, add_index2));
                }
            }
            for (remove_index, remove) in removes.iter().enumerate() {
                if first_add_is_ancestor && index.is_ancestor(remove, add1) {
                    return Some((Some(remove_index), add_index1));
                } else if !first_add_is_ancestor && index.is_ancestor(remove, add2) {
                    return Some((Some(remove_index), add_index2));
                }
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
    let maybe_remote_target = branch_target.remote_targets.get(remote_name);
    if branch_target.local_target.as_ref() == maybe_remote_target {
        return BranchPushAction::AlreadyMatches;
    }

    match (&maybe_remote_target, &branch_target.local_target) {
        (_, Some(RefTarget::Conflict { .. })) => BranchPushAction::LocalConflicted,
        (Some(RefTarget::Conflict { .. }), _) => BranchPushAction::RemoteConflicted,
        (Some(RefTarget::Normal(old_target)), Some(RefTarget::Normal(new_target))) => {
            BranchPushAction::Update(BranchPushUpdate {
                old_target: Some(old_target.clone()),
                new_target: Some(new_target.clone()),
            })
        }
        (Some(RefTarget::Normal(old_target)), None) => BranchPushAction::Update(BranchPushUpdate {
            old_target: Some(old_target.clone()),
            new_target: None,
        }),
        (None, Some(RefTarget::Normal(new_target))) => BranchPushAction::Update(BranchPushUpdate {
            old_target: None,
            new_target: Some(new_target.clone()),
        }),
        (None, None) => {
            panic!("Unexpected branch doesn't exist anywhere")
        }
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
            local_target: Some(RefTarget::Normal(commit_id1.clone())),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::Normal(commit_id1)
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
            local_target: Some(RefTarget::Normal(commit_id1.clone())),
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
            local_target: None,
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::Normal(commit_id1.clone())
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
            local_target: Some(RefTarget::Normal(commit_id2.clone())),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::Normal(commit_id1.clone())
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
            local_target: Some(RefTarget::Conflict {
                removes: vec![],
                adds: vec![commit_id1.clone(), commit_id2],
            }),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::Normal(commit_id1)
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
            local_target: Some(RefTarget::Normal(commit_id1.clone())),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::Conflict {
                removes: vec![],
                adds: vec![commit_id1, commit_id2]
            }
            },
        };
        assert_eq!(
            classify_branch_push_action(&branch, "origin"),
            BranchPushAction::RemoteConflicted
        );
    }
}
